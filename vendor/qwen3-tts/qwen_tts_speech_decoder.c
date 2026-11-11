/*
 * qwen_tts_speech_decoder.c - Speech Decoder (ConvNet) forward pass
 * Converts 16 codebook codes per frame → 24kHz audio waveform
 *
 * Architecture:
 * 1. VQ dequant (16 codebooks × 2048 × 256) → sum → project to 512
 * 2. Pre-conv (512→1024, k=3, causal)
 * 3. Pre-transformer (8 layers, hidden=512, sliding window=72, layer_scale)
 * 4. Output proj (512→1024)
 * 5. ConvNeXt upsample (2 blocks, 2x each)
 * 6. Initial conv (1024→1536, k=7)
 * 7. 4 Decoder upsample blocks (rates: 8,5,4,3) with 3 residual blocks each
 * 8. Final snake + conv (96→1, k=7) → audio
 *
 * Tensor naming from safetensors:
 *   decoder.upsample.{0,1}.0.conv.{weight,bias}        - ConvNeXt ConvTranspose
 *   decoder.upsample.{0,1}.1.{dwconv.conv,norm,...}     - ConvNeXt block
 *   decoder.decoder.0.conv.{weight,bias}                - initial conv
 *   decoder.decoder.{1-4}.block.0.{alpha,beta}          - snake before upsample
 *   decoder.decoder.{1-4}.block.1.conv.{weight,bias}    - ConvTranspose upsample
 *   decoder.decoder.{1-4}.block.{2-4}.{act1,conv1,act2,conv2} - ResBlocks
 *   decoder.decoder.5.{alpha,beta}                      - final snake
 *   decoder.decoder.6.conv.{weight,bias}                - final conv
 */

#include <pthread.h>
#include "qwen_tts.h"
#include "qwen_tts_kernels.h"
#include "ingot/safetensors.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

#ifdef __ARM_NEON
#include <arm_neon.h>
#endif
#ifdef __AVX2__
#include <immintrin.h>
#endif

/* aligned_malloc/aligned_calloc now in qwen_tts_kernels.h */

#ifdef __APPLE__
#include <Accelerate/Accelerate.h>
/* Modern Accelerate (ACCELERATE_NEW_LAPACK, cblas_new.h) declares `enum CBLAS_TRANSPOSE`
 * (C-style tag) rather than the old `typedef ... CBLAS_TRANSPOSE`, so the cast needs the
 * `enum` keyword. OpenBLAS keeps the typedef. Portable alias: */
#define QWEN_CBLAS_TRANSPOSE enum CBLAS_TRANSPOSE
#else
#include <cblas.h>
#define QWEN_CBLAS_TRANSPOSE CBLAS_TRANSPOSE
#endif

/* SD_GEMM: sgemm RowMajor de OpenBLAS/Accelerate; la firma omite el argumento CblasRowMajor. */
static inline void SD_GEMM(int ta,int tb,int M,int N,int K,float al,const float *A,int lda,
                           const float *B,int ldb,float be,float *C,int ldc){
    cblas_sgemm(CblasRowMajor,(QWEN_CBLAS_TRANSPOSE)ta,(QWEN_CBLAS_TRANSPOSE)tb,M,N,K,al,A,lda,B,ldb,be,C,ldc);
}

static const float *get_f32(void *ms, const char *name) {
    const ingot_st_tensor *t = ingot_st_find((ingot_st *)ms, name);
    return t ? (const float *)ingot_st_data((ingot_st *)ms, t) : NULL;
}

/* Causal Conv1d: out_len = (in_len + pad_left - kernel) / stride + 1 */
static int conv1d_out_len(int in_len, int kernel, int stride, int pad_left) {
    return (in_len + pad_left - kernel) / stride + 1;
}

/* Causal ConvTranspose1d: out_len = (in_len-1)*stride + kernel - (kernel-stride) = in_len*stride */
static int conv_transpose1d_out_len(int in_len, int kernel, int stride) {
    return (in_len - 1) * stride + kernel - (kernel - stride);
}

/* Snake activation dispatched through qwen_snake_activation() kernel
 * (NEON/Accelerate-optimized in qwen_tts_kernels.c) */
#define snake_activation qwen_snake_activation

/* ========================================================================
 * INT8 conv path for the decoder (PR #17 sub-change B, ported)
 *
 * OPT-IN ONLY (QWEN_SD_INT8=1) and only on ARM dotprod: it trades audio
 * quality for speed. Measured on Neoverse-N1 (0.6B --int4 -j4): stream RTF
 * 1.41 -> 1.15, decoder 7735 -> 5112 ms. Added noise floor ~-65 dBFS RMS.
 * The ConvTranspose int8 variant is NOT ported: the author measured it slower
 * than fp32 sgemm (small K pads poorly into blocks) and ships it disabled.
 * ======================================================================== */
static int sd_int8_enabled(void) {
    static int en = -1;
    if (en < 0) {
        const char *e = getenv("QWEN_SD_INT8");
        en = (e && *e && *e != '0') && qwen_sd_int8_available();
    }
    return en;
}

/* Scale-block size along K (multiple of 16). Smaller = more accurate, slightly
 * slower; the author measured 64 as the sweet spot on N1 (same RTF as 128,
 * ~4 dB better SNR). QWEN_SD_INT8_BLK overrides. */
static int sd_int8_blk(void) {
    static int blk = -1;
    if (blk < 0) {
        const char *e = getenv("QWEN_SD_INT8_BLK");
        blk = e ? atoi(e) : 64;
        if (blk < 16) blk = 16;
        blk = (blk + 15) & ~15;
    }
    return blk;
}

/* Quantized-weight registry keyed by source pointer. Quantize-once; the entries
 * live until process exit (bounded at SD_WQ_MAX, and only populated when the
 * env flag is on). Weights are read-only and shared across streaming slots. */
typedef struct {
    const float *src;
    int8_t *q;
    float *scales;
    int Kp;
} sd_wq_entry_t;

#define SD_WQ_MAX 128
static sd_wq_entry_t sd_wq[SD_WQ_MAX];
static int sd_wq_n = 0;
static pthread_mutex_t sd_wq_mu = PTHREAD_MUTEX_INITIALIZER;

/* Conv1d weight [out_ch, in_ch*kernel]: rows are already in im2col order. */
static sd_wq_entry_t *sd_wq_get_conv(const float *w, int out_ch, int K) {
    pthread_mutex_lock(&sd_wq_mu);
    for (int i = 0; i < sd_wq_n; i++)
        if (sd_wq[i].src == w) { pthread_mutex_unlock(&sd_wq_mu); return &sd_wq[i]; }
    if (sd_wq_n >= SD_WQ_MAX) { pthread_mutex_unlock(&sd_wq_mu); return NULL; }
    int blk = sd_int8_blk();
    int Kp = qwen_int8_kp(K, blk);
    sd_wq_entry_t *e = &sd_wq[sd_wq_n];
    e->q = (int8_t *)aligned_malloc((size_t)out_ch * Kp);
    e->scales = (float *)aligned_malloc((size_t)out_ch * (Kp / blk) * sizeof(float));
    if (!e->q || !e->scales) { free(e->q); free(e->scales); pthread_mutex_unlock(&sd_wq_mu); return NULL; }
    e->src = w; e->Kp = Kp;
    qwen_int8_quant_rows(e->q, e->scales, w, out_ch, K, Kp, blk);
    sd_wq_n++;
    pthread_mutex_unlock(&sd_wq_mu);
    return e;
}

/* Add bias to channel-first output [channels, length] */
static void conv_add_bias(float *out, const float *bias, int channels, int length) {
    if (!bias) return;
    for (int c = 0; c < channels; c++) {
        float b = bias[c];
        float *row = out + (int64_t)c * length;
        for (int t = 0; t < length; t++)
            row[t] += b;
    }
}

/* Devuelve la ranura `slot` con capacidad para al menos `n` floats. Solo crece
 * (libera el búfer anterior y reserva uno nuevo, sin conservar su contenido) y
 * nunca encoge; devuelve NULL si la reserva falla. El contenido devuelto no está
 * inicializado: el llamador escribe todo el tramo que lee. */
static float *sd_ws_get(qwen_sd_workspace_t *ws, qwen_sd_slot_t slot, size_t n) {
    qwen_sd_slot_buf_t *s = &ws->slot[slot];
    if (n > s->cap) {
        free(s->p);
        s->p = (float *)aligned_malloc(n * sizeof(float));
        s->cap = s->p ? n : 0;
    }
    return s->p;
}

void qwen_sd_workspace_free(qwen_sd_workspace_t *ws) {
    for (int i = 0; i < QWEN_SDW_COUNT; i++) free(ws->slot[i].p);
    memset(ws, 0, sizeof(*ws));
}

/* Reordena el peso de una convolución [out_ch][in_ch][kernel] a [kernel][out_ch][in_ch]
 * en una copia nueva (el original es de solo lectura). Devuelve NULL si falla la reserva. */
static float *conv_reorder_weight(const float *weight, int out_ch, int in_ch, int kernel) {
    float *wr = (float *)aligned_malloc((size_t)kernel * out_ch * in_ch * sizeof(float));
    if (!wr) return NULL;
    for (int k = 0; k < kernel; k++)
        for (int oc = 0; oc < out_ch; oc++)
            for (int ic = 0; ic < in_ch; ic++)
                wr[((int64_t)k * out_ch + oc) * in_ch + ic] =
                    weight[(int64_t)oc * in_ch * kernel + (int64_t)ic * kernel + k];
    return wr;
}

void qwen_speech_decoder_free_conv_weights(qwen_speech_decoder_t *sd) {
    free(sd->pre_conv_wk); sd->pre_conv_wk = NULL;
    free(sd->initial_conv_wk); sd->initial_conv_wk = NULL;
    for (int b = 0; b < 4; b++)
        for (int r = 0; r < 3; r++) {
            free(sd->upsample_blocks[b].res_blocks[r].conv1_wk);
            sd->upsample_blocks[b].res_blocks[r].conv1_wk = NULL;
        }
}

/* Conv1d causal: k>1 con un sgemm acumulado por posición del kernel sobre la entrada
 * con relleno izquierdo; k=1 con sgemm directo. `weight` es el peso original
 * [out_ch, in_ch, kernel] y `weight_k` su reordenado [kernel][out_ch][in_ch] (solo se
 * lee con kernel > 1). Devuelve 0, o -1 si falla la reserva en el espacio de trabajo. */
static int causal_conv1d(qwen_sd_workspace_t *ws, float *out, const float *in,
                          const float *weight, const float *weight_k, const float *bias,
                          int in_ch, int out_ch, int length,
                          int kernel, int dilation) {
    /* int8 only for the upsample-block res convs: the latent-domain convs
     * (pre-conv 512->1024, ConvNeXt) are quality-sensitive and cheap, so they
     * stay fp32. Same scoping as the author's. */
    if (sd_int8_enabled() && in_ch == out_ch && in_ch <= 768) {
        sd_wq_entry_t *e = sd_wq_get_conv(weight, out_ch, in_ch * kernel);
        if (e) {
            qwen_conv1d_int8(out, in, e->q, e->scales, bias,
                             in_ch, out_ch, length, kernel, dilation,
                             e->Kp, sd_int8_blk());
            return 0;
        }
    }

    if (kernel == 1) {
        /* k=1: weight is [out_ch, in_ch], direct matmul */
        SD_GEMM(CblasNoTrans, CblasNoTrans,
                    out_ch, length, in_ch,
                    1.0f, weight, in_ch,
                    in, length,
                    0.0f, out, length);
        conv_add_bias(out, bias, out_ch, length);
        return 0;
    }

    /* k>1: la salida en t suma W_k × x[t - pad_left + k*dilation] sobre k. Con la
     * entrada precedida por pad_left ceros, cada posición k es un sgemm sobre la
     * ventana desplazada k*dilation columnas, acumulado sobre la salida. */
    int pad_left = (kernel - 1) * dilation;
    int ld = pad_left + length;
    float *xp = sd_ws_get(ws, QWEN_SDW_XP, (size_t)in_ch * ld);
    if (!xp) return -1;
    for (int ic = 0; ic < in_ch; ic++) {
        memset(xp + (int64_t)ic * ld, 0, (size_t)pad_left * sizeof(float));
        memcpy(xp + (int64_t)ic * ld + pad_left, in + (int64_t)ic * length,
               (size_t)length * sizeof(float));
    }

    for (int k = 0; k < kernel; k++)
        SD_GEMM(CblasNoTrans, CblasNoTrans,
                    out_ch, length, in_ch,
                    1.0f, weight_k + (int64_t)k * out_ch * in_ch, in_ch,
                    xp + (int64_t)k * dilation, ld,
                    k ? 1.0f : 0.0f, out, length);

    conv_add_bias(out, bias, out_ch, length);
    return 0;
}

/* ConvTranspose1d causal: un único sgemm sobre el peso [in_ch][out_ch*kernel] (ya
 * contiguo así) y dispersión ascendente en k sobre la salida. Devuelve 0, o -1 si
 * falla la reserva del búfer de trabajo. */
static int causal_conv_transpose1d(qwen_sd_workspace_t *ws, float *out, const float *in,
                                    const float *weight, const float *bias,
                                    int in_ch, int out_ch, int in_len, int out_len,
                                    int kernel, int stride) {
    memset(out, 0, (int64_t)out_ch * out_len * sizeof(float));
    /* El recorte derecho lo fija solo out_len: las llamadas de una pasada pasan
     * out_len = in_len*stride (descarta las columnas de cola) y la ruta de streaming
     * pasa out_len = longitud completa para conservar esas columnas como acarreo
     * del overlap-add. */

    /* r[(oc*kernel + k)][t] = suma sobre ic de weight[ic][oc*kernel + k] × in[ic][t] */
    int64_t m = (int64_t)out_ch * kernel;
    float *r = sd_ws_get(ws, QWEN_SDW_CT, (size_t)(m * in_len));
    if (!r) return -1;
    SD_GEMM(CblasTrans, CblasNoTrans,
                (int)m, in_len, in_ch,
                1.0f, weight, (int)m,
                in, in_len,
                0.0f, r, in_len);

    /* Dispersión a las posiciones de salida con zancada, en orden ascendente de k */
    for (int k = 0; k < kernel; k++) {
        for (int oc = 0; oc < out_ch; oc++) {
            const float *src = r + ((int64_t)oc * kernel + k) * in_len;
            float *dst = out + (int64_t)oc * out_len;
            for (int t = 0; t < in_len; t++) {
                int out_pos = t * stride + k;
                if (out_pos < out_len)
                    dst[out_pos] += src[t];
            }
        }
    }

    conv_add_bias(out, bias, out_ch, out_len);
    return 0;
}

/* ========================================================================
 * Weight Loading
 * ======================================================================== */

int qwen_speech_decoder_load(qwen_tts_ctx_t *ctx) {
    qwen_tts_config_t *c = &ctx->config;
    void *ms = ctx->speech_safetensors;
    qwen_speech_decoder_t *sd = &ctx->speech_dec;

    if (!ctx->silent) fprintf(stderr, "Loading Speech Decoder weights...\n");

    int cb_dim = QWEN_TTS_CODEBOOK_DIM;
    int cb_size = c->codebook_size;

    /* Codebook 0 (rvq_first) - dequantize from EMA */
    const float *emb_sum = get_f32(ms, "decoder.quantizer.rvq_first.vq.layers.0._codebook.embedding_sum");
    const float *usage = get_f32(ms, "decoder.quantizer.rvq_first.vq.layers.0._codebook.cluster_usage");
    if (emb_sum && usage) {
        sd->codebook[0] = (float *)aligned_malloc((int64_t)cb_size * cb_dim * sizeof(float));
        for (int i = 0; i < cb_size; i++)
            for (int d = 0; d < cb_dim; d++)
                sd->codebook[0][(int64_t)i * cb_dim + d] = emb_sum[(int64_t)i * cb_dim + d] / fmaxf(usage[i], 1e-5f);
    }

    /* Codebooks 1-15 (rvq_rest) */
    for (int k = 0; k < 15; k++) {
        char es_name[128], cu_name[128];
        snprintf(es_name, sizeof(es_name), "decoder.quantizer.rvq_rest.vq.layers.%d._codebook.embedding_sum", k);
        snprintf(cu_name, sizeof(cu_name), "decoder.quantizer.rvq_rest.vq.layers.%d._codebook.cluster_usage", k);
        emb_sum = get_f32(ms, es_name);
        usage = get_f32(ms, cu_name);
        if (emb_sum && usage) {
            sd->codebook[k + 1] = (float *)aligned_malloc((int64_t)cb_size * cb_dim * sizeof(float));
            for (int i = 0; i < cb_size; i++)
                for (int d = 0; d < cb_dim; d++)
                    sd->codebook[k + 1][(int64_t)i * cb_dim + d] = emb_sum[(int64_t)i * cb_dim + d] / fmaxf(usage[i], 1e-5f);
        }
    }

    /* VQ projections */
    sd->rvq_first_output_proj = get_f32(ms, "decoder.quantizer.rvq_first.output_proj.weight");
    sd->rvq_rest_output_proj = get_f32(ms, "decoder.quantizer.rvq_rest.output_proj.weight");

    /* Pre-conv */
    sd->pre_conv_weight = get_f32(ms, "decoder.pre_conv.conv.weight");
    sd->pre_conv_bias = get_f32(ms, "decoder.pre_conv.conv.bias");

    /* Pre-transformer */
    sd->input_proj_weight = get_f32(ms, "decoder.pre_transformer.input_proj.weight");
    sd->input_proj_bias = get_f32(ms, "decoder.pre_transformer.input_proj.bias");
    sd->final_norm_weight = get_f32(ms, "decoder.pre_transformer.norm.weight");
    sd->output_proj_weight = get_f32(ms, "decoder.pre_transformer.output_proj.weight");
    sd->output_proj_bias = get_f32(ms, "decoder.pre_transformer.output_proj.bias");

    sd->pre_layers = (qwen_sd_pre_layer_t *)calloc(c->dec_num_layers, sizeof(qwen_sd_pre_layer_t));
    for (int i = 0; i < c->dec_num_layers; i++) {
        qwen_sd_pre_layer_t *l = &sd->pre_layers[i];
        char name[128];
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.input_layernorm.weight", i);
        l->attn_norm = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.self_attn.q_proj.weight", i);
        l->attn_q = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.self_attn.k_proj.weight", i);
        l->attn_k = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.self_attn.v_proj.weight", i);
        l->attn_v = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.self_attn.o_proj.weight", i);
        l->attn_o = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.self_attn_layer_scale.scale", i);
        l->attn_layer_scale = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.post_attention_layernorm.weight", i);
        l->ffn_norm = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.mlp.gate_proj.weight", i);
        l->ffn_gate = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.mlp.up_proj.weight", i);
        l->ffn_up = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.mlp.down_proj.weight", i);
        l->ffn_down = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.pre_transformer.layers.%d.mlp_layer_scale.scale", i);
        l->ffn_layer_scale = get_f32(ms, name);
    }

    /* RoPE cache for pre-transformer (NeoX split-half) */
    int half_dim = c->dec_head_dim / 2;
    sd->rope_cos = (float *)aligned_malloc(8000 * half_dim * sizeof(float));
    sd->rope_sin = (float *)aligned_malloc(8000 * half_dim * sizeof(float));
    for (int pos = 0; pos < 8000; pos++) {
        for (int i = 0; i < half_dim; i++) {
            float angle = pos / powf(c->dec_rope_theta, (float)(2*i) / c->dec_head_dim);
            sd->rope_cos[pos * half_dim + i] = cosf(angle);
            sd->rope_sin[pos * half_dim + i] = sinf(angle);
        }
    }

    /* ConvNeXt upsample blocks (2 blocks) */
    for (int b = 0; b < 2; b++) {
        qwen_sd_convnext_t *cn = &sd->convnext[b];
        char name[128];
        /* ConvTranspose1d is sub-layer 0 */
        snprintf(name, sizeof(name), "decoder.upsample.%d.0.conv.weight", b);
        cn->conv_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.0.conv.bias", b);
        cn->conv_bias = get_f32(ms, name);
        /* ConvNeXt block is sub-layer 1 */
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.dwconv.conv.weight", b);
        cn->dwconv_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.dwconv.conv.bias", b);
        cn->dwconv_bias = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.pwconv1.weight", b);
        cn->pwconv1_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.pwconv1.bias", b);
        cn->pwconv1_bias = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.pwconv2.weight", b);
        cn->pwconv2_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.pwconv2.bias", b);
        cn->pwconv2_bias = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.norm.weight", b);
        cn->norm_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.norm.bias", b);
        cn->norm_bias = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.upsample.%d.1.gamma", b);
        cn->gamma = get_f32(ms, name);
    }

    /* Initial conv: decoder.decoder.0 */
    sd->initial_conv_weight = get_f32(ms, "decoder.decoder.0.conv.weight");
    sd->initial_conv_bias = get_f32(ms, "decoder.decoder.0.conv.bias");

    /* Decoder upsample blocks: decoder.decoder.{1-4} */
    for (int b = 0; b < 4; b++) {
        qwen_sd_upsample_block_t *ub = &sd->upsample_blocks[b];
        int bi = b + 1; /* tensor index: 1-4 */
        char name[128];

        /* Snake before upsample: block.0.{alpha,beta} */
        snprintf(name, sizeof(name), "decoder.decoder.%d.block.0.alpha", bi);
        ub->upsample.snake_alpha = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.decoder.%d.block.0.beta", bi);
        ub->upsample.snake_beta = get_f32(ms, name);

        /* ConvTranspose upsample: block.1.conv.{weight,bias} */
        snprintf(name, sizeof(name), "decoder.decoder.%d.block.1.conv.weight", bi);
        ub->upsample.conv_weight = get_f32(ms, name);
        snprintf(name, sizeof(name), "decoder.decoder.%d.block.1.conv.bias", bi);
        ub->upsample.conv_bias = get_f32(ms, name);

        /* 3 residual blocks: block.{2,3,4} */
        for (int r = 0; r < 3; r++) {
            int ri = r + 2; /* tensor index: 2,3,4 */
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.act1.alpha", bi, ri);
            ub->res_blocks[r].snake1_alpha = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.act1.beta", bi, ri);
            ub->res_blocks[r].snake1_beta = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.conv1.conv.weight", bi, ri);
            ub->res_blocks[r].conv1_weight = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.conv1.conv.bias", bi, ri);
            ub->res_blocks[r].conv1_bias = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.act2.alpha", bi, ri);
            ub->res_blocks[r].snake2_alpha = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.act2.beta", bi, ri);
            ub->res_blocks[r].snake2_beta = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.conv2.conv.weight", bi, ri);
            ub->res_blocks[r].conv2_weight = get_f32(ms, name);
            snprintf(name, sizeof(name), "decoder.decoder.%d.block.%d.conv2.conv.bias", bi, ri);
            ub->res_blocks[r].conv2_bias = get_f32(ms, name);
        }
    }

    /* Final snake: decoder.decoder.5 */
    sd->final_snake.alpha = get_f32(ms, "decoder.decoder.5.alpha");
    sd->final_snake.beta = get_f32(ms, "decoder.decoder.5.beta");

    /* Final conv: decoder.decoder.6 */
    sd->final_conv_weight = get_f32(ms, "decoder.decoder.6.conv.weight");
    sd->final_conv_bias = get_f32(ms, "decoder.decoder.6.conv.bias");

    /* Copias reordenadas [kernel][out_ch][in_ch] de las convoluciones de kernel > 1,
     * compartidas por todos los clones de worker. */
    if (sd->pre_conv_weight) {
        sd->pre_conv_wk = conv_reorder_weight(sd->pre_conv_weight, 1024, 512, 3);
        if (!sd->pre_conv_wk) goto reorder_fail;
    }
    if (sd->initial_conv_weight) {
        sd->initial_conv_wk = conv_reorder_weight(sd->initial_conv_weight, 1536, 1024, 7);
        if (!sd->initial_conv_wk) goto reorder_fail;
    }
    {
        static const int res_ch[4] = {768, 384, 192, 96};
        for (int b = 0; b < 4; b++)
            for (int r = 0; r < 3; r++) {
                qwen_sd_upsample_block_t *ub = &sd->upsample_blocks[b];
                if (!ub->res_blocks[r].conv1_weight) continue;
                ub->res_blocks[r].conv1_wk = conv_reorder_weight(ub->res_blocks[r].conv1_weight,
                                                                 res_ch[b], res_ch[b], 7);
                if (!ub->res_blocks[r].conv1_wk) goto reorder_fail;
            }
    }

    /* Debug: verify pre_conv weights right after loading */
    if (sd->pre_conv_weight) {
        fprintf(stderr, "  [LOAD] pre_conv_w[:5]: [%.6f, %.6f, %.6f, %.6f, %.6f] bias[:3]: [%.6f, %.6f, %.6f]\n",
                sd->pre_conv_weight[0], sd->pre_conv_weight[1], sd->pre_conv_weight[2],
                sd->pre_conv_weight[3], sd->pre_conv_weight[4],
                sd->pre_conv_bias[0], sd->pre_conv_bias[1], sd->pre_conv_bias[2]);
    } else {
        fprintf(stderr, "  [LOAD] pre_conv_weight is NULL!\n");
    }

    if (!ctx->silent) {
        fprintf(stderr, "  Codebooks: 16/16 (dequantized from EMA)\n");
        fprintf(stderr, "  Pre-transformer: %d layers, input_proj=%s\n",
                c->dec_num_layers, sd->input_proj_weight ? "ok" : "MISSING");
        fprintf(stderr, "  ConvNeXt upsample: %s\n",
                (sd->convnext[0].conv_weight && sd->convnext[1].conv_weight) ? "ok" : "MISSING");
        fprintf(stderr, "  Conv decoder: initial=%s, final=%s\n",
                sd->initial_conv_weight ? "ok" : "MISSING",
                sd->final_conv_weight ? "ok" : "MISSING");
        fprintf(stderr, "  Upsample blocks: [%s %s %s %s]\n",
                sd->upsample_blocks[0].upsample.conv_weight ? "ok" : "MISSING",
                sd->upsample_blocks[1].upsample.conv_weight ? "ok" : "MISSING",
                sd->upsample_blocks[2].upsample.conv_weight ? "ok" : "MISSING",
                sd->upsample_blocks[3].upsample.conv_weight ? "ok" : "MISSING");
        fprintf(stderr, "  Final snake: %s\n",
                sd->final_snake.alpha ? "ok" : "MISSING");
    }

    return 0;

reorder_fail:
    fprintf(stderr, "Error: sin memoria para reordenar los pesos de convolución del decodificador\n");
    qwen_speech_decoder_free_conv_weights(sd);
    return -1;
}

/* ========================================================================
 * Decode: codes → audio
 * ======================================================================== */

static int conv_decoder_forward(qwen_tts_ctx_t *ctx, qwen_sd_workspace_t *ws,
                                 float *signal, int cur_ch, int cur_len,
                                 float **audio_out, int *n_samples_out);

int qwen_speech_decoder_decode(qwen_tts_ctx_t *ctx, const int *codes, int n_frames,
                                float **audio_out, int *n_samples) {
    qwen_speech_decoder_t *sd = &ctx->speech_dec;
    qwen_tts_config_t *c = &ctx->config;

    int cb_dim = QWEN_TTS_CODEBOOK_DIM;
    int vq_hidden = 512;
    int latent_dim = 1024;

    if (!ctx->silent)
        fprintf(stderr, "  Speech decoder: %d frames -> audio...\n", n_frames);

    /* Debug: check if weights are still intact */
    if (ctx->debug && sd->pre_conv_weight) {
        fprintf(stderr, "[DECODER] ENTRY pre_conv_w[:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                sd->pre_conv_weight[0], sd->pre_conv_weight[1], sd->pre_conv_weight[2],
                sd->pre_conv_weight[3], sd->pre_conv_weight[4]);
    }

    /* Step 1: VQ dequant + output projection (batched with BLAS) */
    float *vq_out = (float *)aligned_calloc((int64_t)n_frames * vq_hidden, sizeof(float));

    /* Gather codebook embeddings into matrices for batched projection */
    float *emb_first = (float *)aligned_malloc((int64_t)n_frames * cb_dim * sizeof(float));
    float *emb_rest = (float *)aligned_calloc((int64_t)n_frames * cb_dim, sizeof(float));

    for (int f = 0; f < n_frames; f++) {
        /* Codebook 0 (rvq_first) */
        int code0 = codes[f * 16];
        if (code0 >= 0 && code0 < c->codebook_size && sd->codebook[0]) {
            memcpy(emb_first + (int64_t)f * cb_dim,
                   sd->codebook[0] + (int64_t)code0 * cb_dim, cb_dim * sizeof(float));
        } else {
            memset(emb_first + (int64_t)f * cb_dim, 0, cb_dim * sizeof(float));
        }

        /* Codebooks 1-15 (rvq_rest): sum embeddings */
        float *rest_row = emb_rest + (int64_t)f * cb_dim;
        for (int k = 1; k < 16; k++) {
            int code = codes[f * 16 + k];
            if (code >= 0 && code < c->codebook_size && sd->codebook[k]) {
                const float *emb = sd->codebook[k] + (int64_t)code * cb_dim;
                for (int d = 0; d < cb_dim; d++) rest_row[d] += emb[d];
            }
        }
    }

    /* Batched projection: vq_out[n_frames, vq_hidden] = emb[n_frames, cb_dim] × W^T[cb_dim, vq_hidden] */
    if (sd->rvq_first_output_proj) {
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    n_frames, vq_hidden, cb_dim, 1.0f,
                    emb_first, cb_dim, sd->rvq_first_output_proj, cb_dim,
                    0.0f, vq_out, vq_hidden);
    }
    if (sd->rvq_rest_output_proj) {
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    n_frames, vq_hidden, cb_dim, 1.0f,
                    emb_rest, cb_dim, sd->rvq_rest_output_proj, cb_dim,
                    1.0f, vq_out, vq_hidden);  /* accumulate */
    }
    free(emb_first); free(emb_rest);

    /* Debug: dump first frame's RVQ output */
    if (ctx->debug) {
        float rms0 = 0;
        for (int i = 0; i < vq_hidden; i++) rms0 += vq_out[i] * vq_out[i];
        rms0 = sqrtf(rms0 / vq_hidden);
        fprintf(stderr, "[DECODER] RVQ out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f] RMS=%.6f\n",
                vq_out[0], vq_out[1], vq_out[2], vq_out[3], vq_out[4], rms0);
        /* Also check the transposed buffer */
    }

    /* Step 2: Pre-conv (512→1024, k=3, causal, pad_left=2) */
    /* Need channel-first format for conv: [vq_hidden, n_frames] */
    float *vq_cf = (float *)aligned_malloc((int64_t)vq_hidden * n_frames * sizeof(float));
    for (int f = 0; f < n_frames; f++)
        for (int d = 0; d < vq_hidden; d++)
            vq_cf[(int64_t)d * n_frames + f] = vq_out[(int64_t)f * vq_hidden + d];
    free(vq_out);

    /* Debug: check transposed buffer */
    if (ctx->debug) {
        float rms_cf = 0;
        for (int d = 0; d < vq_hidden; d++) {
            float v = vq_cf[(int64_t)d * n_frames + 0]; /* frame 0, channel d */
            rms_cf += v * v;
        }
        rms_cf = sqrtf(rms_cf / vq_hidden);
        fprintf(stderr, "[DECODER] vq_cf frame 0 ch[:5]: [%.6f, %.6f, %.6f, %.6f, %.6f] RMS=%.6f\n",
                vq_cf[0 * n_frames + 0], vq_cf[1 * n_frames + 0],
                vq_cf[2 * n_frames + 0], vq_cf[3 * n_frames + 0],
                vq_cf[4 * n_frames + 0], rms_cf);
        /* Check pre_conv weight */
        fprintf(stderr, "[DECODER] pre_conv_w[:5]: [%.6f, %.6f, %.6f, %.6f, %.6f] bias[:3]: [%.6f, %.6f, %.6f]\n",
                sd->pre_conv_weight[0], sd->pre_conv_weight[1], sd->pre_conv_weight[2],
                sd->pre_conv_weight[3], sd->pre_conv_weight[4],
                sd->pre_conv_bias[0], sd->pre_conv_bias[1], sd->pre_conv_bias[2]);
    }

    float *pre_conv_out = (float *)aligned_calloc((int64_t)latent_dim * n_frames, sizeof(float));
    if (causal_conv1d(&ctx->sd_ws, pre_conv_out, vq_cf, sd->pre_conv_weight, sd->pre_conv_wk, sd->pre_conv_bias,
                      vq_hidden, latent_dim, n_frames, 3, 1) != 0) {
        free(pre_conv_out); free(vq_cf);
        return -1;
    }
    free(vq_cf);

    /* Debug: dump pre_conv output */
    if (ctx->debug) {
        fprintf(stderr, "[DECODER] pre_conv out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                pre_conv_out[0 * n_frames + 0], pre_conv_out[1 * n_frames + 0],
                pre_conv_out[2 * n_frames + 0], pre_conv_out[3 * n_frames + 0],
                pre_conv_out[4 * n_frames + 0]);
    }

    /* Step 3: Transpose to row-major + input_proj (1024→512) */
    int dec_hidden = 512;
    float *hidden = (float *)aligned_malloc((int64_t)n_frames * dec_hidden * sizeof(float));
    /* Transpose pre_conv_out from channel-first [1024, n_frames] to row-major [n_frames, 1024] */
    float *pre_conv_rm = (float *)aligned_malloc((int64_t)n_frames * latent_dim * sizeof(float));
    for (int f = 0; f < n_frames; f++)
        for (int d = 0; d < latent_dim; d++)
            pre_conv_rm[(int64_t)f * latent_dim + d] = pre_conv_out[(int64_t)d * n_frames + f];
    free(pre_conv_out);
    /* hidden[n_frames, 512] = pre_conv_rm[n_frames, 1024] × W^T[1024, 512] */
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                n_frames, dec_hidden, latent_dim, 1.0f,
                pre_conv_rm, latent_dim,
                sd->input_proj_weight, latent_dim,
                0.0f, hidden, dec_hidden);
    free(pre_conv_rm);
    if (sd->input_proj_bias) {
        for (int f = 0; f < n_frames; f++)
            for (int o = 0; o < dec_hidden; o++)
                hidden[(int64_t)f * dec_hidden + o] += sd->input_proj_bias[o];
    }

    /* Debug: dump input_proj output */
    if (ctx->debug) {
        fprintf(stderr, "[DECODER] input_proj out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                hidden[0], hidden[1], hidden[2], hidden[3], hidden[4]);
    }

    /* Step 4: Pre-transformer (8 layers with sliding window causal attention) */
    int dec_inter = 1024;
    int n_heads = 16;
    int head_dim = c->dec_head_dim; /* 64 */
    int qkv_dim = n_heads * head_dim;
    int window = 72;
    float eps = c->dec_rms_norm_eps;
    int half_hd = head_dim / 2;

    float *q = (float *)aligned_malloc((int64_t)n_frames * qkv_dim * sizeof(float));
    float *kk = (float *)aligned_malloc((int64_t)n_frames * qkv_dim * sizeof(float));
    float *vv = (float *)aligned_malloc((int64_t)n_frames * qkv_dim * sizeof(float));
    float *x_norm = (float *)aligned_malloc((int64_t)n_frames * dec_hidden * sizeof(float));
    float *attn_out = (float *)aligned_malloc((int64_t)n_frames * qkv_dim * sizeof(float));

    for (int layer = 0; layer < c->dec_num_layers; layer++) {
        qwen_sd_pre_layer_t *l = &sd->pre_layers[layer];

        /* Input RMSNorm (NEON-optimized) */
        qwen_rms_norm(x_norm, hidden, l->attn_norm, n_frames, dec_hidden, eps);

        /* Debug after input RMSNorm for layer 0 */
        if (ctx->debug && layer == 0) {
            fprintf(stderr, "[DECODER] Layer 0 input_norm frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    x_norm[0], x_norm[1], x_norm[2], x_norm[3], x_norm[4]);
        }

        /* QKV projections */
        /* x_norm[n_frames, dec_hidden] × W^T = out[n_frames, qkv_dim] */
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    n_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_q, dec_hidden, 0.0f, q, qkv_dim);
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    n_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_k, dec_hidden, 0.0f, kk, qkv_dim);
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    n_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_v, dec_hidden, 0.0f, vv, qkv_dim);

        /* Debug QKV for layer 0 */
        if (ctx->debug && layer == 0) {
            fprintf(stderr, "[DECODER] Layer 0 Q frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    q[0], q[1], q[2], q[3], q[4]);
            fprintf(stderr, "[DECODER] Layer 0 K frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    kk[0], kk[1], kk[2], kk[3], kk[4]);
            fprintf(stderr, "[DECODER] Layer 0 V frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    vv[0], vv[1], vv[2], vv[3], vv[4]);
        }

        /* NeoX split-half RoPE (NEON-optimized, NO QK-norm for pre-transformer) */
        for (int s = 0; s < n_frames; s++) {
            const float *cos_ptr = sd->rope_cos + s * half_hd;
            const float *sin_ptr = sd->rope_sin + s * half_hd;
            for (int hh = 0; hh < n_heads; hh++) {
                float *qh = q + s * qkv_dim + hh * head_dim;
                float *kh = kk + s * qkv_dim + hh * head_dim;
#ifdef __ARM_NEON
                int i = 0;
                for (; i + 3 < half_hd; i += 4) {
                    float32x4_t c = vld1q_f32(cos_ptr + i);
                    float32x4_t si = vld1q_f32(sin_ptr + i);
                    float32x4_t q1 = vld1q_f32(qh + i), q2 = vld1q_f32(qh + i + half_hd);
                    float32x4_t k1 = vld1q_f32(kh + i), k2 = vld1q_f32(kh + i + half_hd);
                    vst1q_f32(qh + i,           vmlsq_f32(vmulq_f32(q1, c), q2, si));
                    vst1q_f32(qh + i + half_hd, vmlaq_f32(vmulq_f32(q2, c), q1, si));
                    vst1q_f32(kh + i,           vmlsq_f32(vmulq_f32(k1, c), k2, si));
                    vst1q_f32(kh + i + half_hd, vmlaq_f32(vmulq_f32(k2, c), k1, si));
                }
                for (; i < half_hd; i++) {
                    float qv1 = qh[i], qv2 = qh[i + half_hd];
                    float kv1 = kh[i], kv2 = kh[i + half_hd];
                    float co = cos_ptr[i], sn = sin_ptr[i];
                    qh[i] = qv1 * co - qv2 * sn; qh[i + half_hd] = qv2 * co + qv1 * sn;
                    kh[i] = kv1 * co - kv2 * sn; kh[i + half_hd] = kv2 * co + kv1 * sn;
                }
#elif defined(__AVX2__)
                int i = 0;
                for (; i + 8 <= half_hd; i += 8) {
                    __m256 c = _mm256_loadu_ps(cos_ptr + i);
                    __m256 si = _mm256_loadu_ps(sin_ptr + i);
                    __m256 q1 = _mm256_loadu_ps(qh + i), q2 = _mm256_loadu_ps(qh + i + half_hd);
                    __m256 k1 = _mm256_loadu_ps(kh + i), k2 = _mm256_loadu_ps(kh + i + half_hd);
                    _mm256_storeu_ps(qh + i,           _mm256_fmsub_ps(q1, c, _mm256_mul_ps(q2, si)));
                    _mm256_storeu_ps(qh + i + half_hd, _mm256_fmadd_ps(q2, c, _mm256_mul_ps(q1, si)));
                    _mm256_storeu_ps(kh + i,           _mm256_fmsub_ps(k1, c, _mm256_mul_ps(k2, si)));
                    _mm256_storeu_ps(kh + i + half_hd, _mm256_fmadd_ps(k2, c, _mm256_mul_ps(k1, si)));
                }
                for (; i < half_hd; i++) {
                    float qv1 = qh[i], qv2 = qh[i + half_hd];
                    float kv1 = kh[i], kv2 = kh[i + half_hd];
                    float co = cos_ptr[i], sn = sin_ptr[i];
                    qh[i] = qv1 * co - qv2 * sn; qh[i + half_hd] = qv2 * co + qv1 * sn;
                    kh[i] = kv1 * co - kv2 * sn; kh[i + half_hd] = kv2 * co + kv1 * sn;
                }
#else
                for (int i = 0; i < half_hd; i++) {
                    float q1 = qh[i], q2 = qh[i + half_hd];
                    float k1 = kh[i], k2 = kh[i + half_hd];
                    float co = cos_ptr[i], si = sin_ptr[i];
                    qh[i] = q1 * co - q2 * si; qh[i + half_hd] = q2 * co + q1 * si;
                    kh[i] = k1 * co - k2 * si; kh[i + half_hd] = k2 * co + k1 * si;
                }
#endif
            }
        }

        /* Sliding window causal attention (NEON-optimized) */
        float scale = 1.0f / sqrtf((float)head_dim);
        qwen_causal_attention_windowed(attn_out, q, kk, vv,
                                        n_frames, n_frames, n_heads, n_heads,
                                        head_dim, scale, 0, window);

        /* Output proj + layer_scale + residual */
        {
            /* proj[n_frames, dec_hidden] = attn_out[n_frames, qkv_dim] × attn_o^T */
            float *oproj = x_norm; /* reuse x_norm as temp (same size: n_frames * dec_hidden) */
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        n_frames, dec_hidden, qkv_dim, 1.0f,
                        attn_out, qkv_dim, l->attn_o, qkv_dim,
                        0.0f, oproj, dec_hidden);
            for (int s = 0; s < n_frames; s++) {
                float *xs = hidden + s * dec_hidden;
                float *ps = oproj + s * dec_hidden;
                if (l->attn_layer_scale) {
                    for (int o = 0; o < dec_hidden; o++)
                        xs[o] += ps[o] * l->attn_layer_scale[o];
                } else {
                    for (int o = 0; o < dec_hidden; o++)
                        xs[o] += ps[o];
                }
            }
        }

        /* Debug after attention + residual */
        if (ctx->debug && layer == 0) {
            fprintf(stderr, "[DECODER] Layer 0 after attn+res frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    hidden[0], hidden[1], hidden[2], hidden[3], hidden[4]);
        }

        /* Post-attn RMSNorm (NEON-optimized) */
        qwen_rms_norm(x_norm, hidden, l->ffn_norm, n_frames, dec_hidden, eps);

        /* SwiGLU FFN: down_proj(SiLU(gate_proj(x)) * up_proj(x)) + layer_scale + residual */
        {
            float *ffn_gate = (float *)aligned_malloc((int64_t)n_frames * dec_inter * sizeof(float));
            float *ffn_up = (float *)aligned_malloc((int64_t)n_frames * dec_inter * sizeof(float));
            /* gate[n_frames, dec_inter] = x_norm[n_frames, dec_hidden] × W_gate^T */
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        n_frames, dec_inter, dec_hidden, 1.0f,
                        x_norm, dec_hidden, l->ffn_gate, dec_hidden,
                        0.0f, ffn_gate, dec_inter);
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        n_frames, dec_inter, dec_hidden, 1.0f,
                        x_norm, dec_hidden, l->ffn_up, dec_hidden,
                        0.0f, ffn_up, dec_inter);
            /* SiLU(gate) * up */
            for (int64_t i = 0; i < (int64_t)n_frames * dec_inter; i++)
                ffn_gate[i] = (ffn_gate[i] / (1.0f + expf(-ffn_gate[i]))) * ffn_up[i];
            free(ffn_up);
            /* down[n_frames, dec_hidden] = ffn_gate[n_frames, dec_inter] × W_down^T */
            float *ffn_down_out = ffn_up = (float *)aligned_malloc((int64_t)n_frames * dec_hidden * sizeof(float));
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        n_frames, dec_hidden, dec_inter, 1.0f,
                        ffn_gate, dec_inter, l->ffn_down, dec_inter,
                        0.0f, ffn_down_out, dec_hidden);
            free(ffn_gate);
            /* layer_scale + residual */
            for (int s = 0; s < n_frames; s++) {
                float *hs = hidden + s * dec_hidden;
                float *ds = ffn_down_out + s * dec_hidden;
                if (l->ffn_layer_scale) {
                    for (int o = 0; o < dec_hidden; o++)
                        hs[o] += ds[o] * l->ffn_layer_scale[o];
                } else {
                    for (int o = 0; o < dec_hidden; o++)
                        hs[o] += ds[o];
                }
            }
            free(ffn_down_out);
        }

        /* Per-layer debug */
        if (ctx->debug) {
            fprintf(stderr, "[DECODER] Layer %d out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                    layer, hidden[0], hidden[1], hidden[2], hidden[3], hidden[4]);
        }
    }

    free(q); free(kk); free(vv); free(x_norm); free(attn_out);

    /* Debug: after pre-transformer */
    if (ctx->debug) {
        fprintf(stderr, "[DECODER] pre-trans out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                hidden[0], hidden[1], hidden[2], hidden[3], hidden[4]);
    }

    /* Step 5: Final RMSNorm + Output proj (512→1024) */
    if (sd->final_norm_weight) {
        qwen_rms_norm(hidden, hidden, sd->final_norm_weight, n_frames, dec_hidden, eps);
    }

    if (ctx->debug) {
        fprintf(stderr, "[DECODER] final_norm frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                hidden[0], hidden[1], hidden[2], hidden[3], hidden[4]);
    }

    float *latent_out = (float *)aligned_malloc((int64_t)n_frames * latent_dim * sizeof(float));
    /* latent_out[n_frames, 1024] = hidden[n_frames, 512] × W^T[512, 1024] */
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                n_frames, latent_dim, dec_hidden, 1.0f,
                hidden, dec_hidden,
                sd->output_proj_weight, dec_hidden,
                0.0f, latent_out, latent_dim);
    if (sd->output_proj_bias) {
        for (int f = 0; f < n_frames; f++)
            for (int o = 0; o < latent_dim; o++)
                latent_out[(int64_t)f * latent_dim + o] += sd->output_proj_bias[o];
    }
    free(hidden);

    /* Step 6: Transpose to channel-first [1024, n_frames] */
    float *signal = (float *)aligned_malloc((int64_t)latent_dim * n_frames * sizeof(float));
    for (int f = 0; f < n_frames; f++)
        for (int d = 0; d < latent_dim; d++)
            signal[(int64_t)d * n_frames + f] = latent_out[(int64_t)f * latent_dim + d];
    free(latent_out);

    /* Debug: after output_proj */
    if (ctx->debug) {
        fprintf(stderr, "[DECODER] output_proj out frame 0 [:5]: [%.6f, %.6f, %.6f, %.6f, %.6f]\n",
                signal[0 * n_frames + 0], signal[1 * n_frames + 0],
                signal[2 * n_frames + 0], signal[3 * n_frames + 0],
                signal[4 * n_frames + 0]);
    }

    /* Step 7: red convolucional (ConvNeXt, conv inicial, bloques de upsample y conv final) */
    float *audio = NULL;
    int audio_len = 0;
    /* La decodificación completa trabaja con la longitud íntegra de la frase: usa
     * un espacio de trabajo local para no dejar en el contexto ranuras del tamaño
     * de toda la frase una vez terminada. */
    qwen_sd_workspace_t ws_full;
    memset(&ws_full, 0, sizeof(ws_full));
    int fwd_rc = conv_decoder_forward(ctx, &ws_full, signal, latent_dim, n_frames, &audio, &audio_len);
    qwen_sd_workspace_free(&ws_full);
    if (fwd_rc != 0)
        return -1;

    *audio_out = audio;
    *n_samples = audio_len;

    if (!ctx->silent)
        fprintf(stderr, "  Speech decoder output: %d samples (%.2fs @ 24kHz)\n",
                audio_len, (float)audio_len / 24000.0f);

    return 0;
}

/* ========================================================================
 * Streaming Incremental Decode
 *
 * En lugar de redecodificar todas las tramas acumuladas en cada fragmento
 * (O(n²)), se procesan solo las tramas NUEVAS por VQ → pre-transformer (con
 * caché KV), se guarda la salida latente y se ejecuta la red convolucional con
 * estado sobre las tramas nuevas, con coste O(1) por fragmento.
 *
 * La salida de audio es exactamente de 1920 muestras por trama del códec (por
 * diseño: upsample 4×480×).
 * ======================================================================== */

/* Initialize/reset streaming state */
void qwen_sd_stream_init(qwen_sd_stream_state_t *st) {
    memset(st, 0, sizeof(*st));
}

/* Free streaming state buffers */
void qwen_sd_stream_free(qwen_sd_stream_state_t *st) {
    for (int i = 0; i < QWEN_SD_STREAM_MAX_LAYERS; i++) {
        free(st->k_cache[i]); st->k_cache[i] = NULL;
        free(st->v_cache[i]); st->v_cache[i] = NULL;
    }
    free(st->latent_cache); st->latent_cache = NULL;
    free(st->vq_pad); st->vq_pad = NULL;
    /* Exact-streaming conv state (may be NULL if the stream never decoded). */
    for (int b = 0; b < 2; b++) { free(st->cs_cn_dw_tail[b]); st->cs_cn_dw_tail[b] = NULL; }
    free(st->cs_init_tail);  st->cs_init_tail = NULL;
    free(st->cs_final_tail); st->cs_final_tail = NULL;
    for (int b = 0; b < 4; b++) {
        free(st->cs_up_carry[b]); st->cs_up_carry[b] = NULL;
        for (int r = 0; r < 3; r++) { free(st->cs_res_tail[b][r]); st->cs_res_tail[b][r] = NULL; }
    }
    st->cs_alloc = 0;
    memset(st, 0, sizeof(*st));
}

/* Cola de un bloque ConvNeXt: LayerNorm -> pw1(+GELU) -> pw2 -> gamma*x + residual,
 * in situ sobre `signal` [cur_ch, cur_len]. Cada paso es por instante de tiempo,
 * sin estado entre fragmentos, así que la comparten la decodificación completa
 * y la de streaming. */
static int convnext_mlp(qwen_sd_workspace_t *ws, qwen_sd_convnext_t *cn, float *signal,
                        const float *residual, int cur_ch, int cur_len) {
    /* LayerNorm (per-timestep) */
    for (int t = 0; t < cur_len; t++) {
        float mean = 0, var = 0;
        for (int c = 0; c < cur_ch; c++) mean += signal[(int64_t)c * cur_len + t];
        mean /= cur_ch;
        for (int c = 0; c < cur_ch; c++) {
            float d = signal[(int64_t)c * cur_len + t] - mean;
            var += d * d;
        }
        var = 1.0f / sqrtf(var / cur_ch + 1e-5f);
        for (int c = 0; c < cur_ch; c++) {
            float x = (signal[(int64_t)c * cur_len + t] - mean) * var;
            signal[(int64_t)c * cur_len + t] = x * cn->norm_weight[c] + cn->norm_bias[c];
        }
    }

    /* Pointwise convs: pw1 (1024→4096, GELU), pw2 (4096→1024) */
    int pw_dim = 4096;
    float *pw1_out = sd_ws_get(ws, QWEN_SDW_PW1, (size_t)pw_dim * cur_len);
    if (!pw1_out) return -1;
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasNoTrans,
                pw_dim, cur_len, cur_ch, 1.0f,
                cn->pwconv1_weight, cur_ch, signal, cur_len,
                0.0f, pw1_out, cur_len);
    if (cn->pwconv1_bias)
        for (int i = 0; i < pw_dim; i++)
            for (int t = 0; t < cur_len; t++)
                pw1_out[(int64_t)i * cur_len + t] += cn->pwconv1_bias[i];
    /* Exact GELU */
    for (int64_t i = 0; i < (int64_t)pw_dim * cur_len; i++)
        pw1_out[i] = 0.5f * pw1_out[i] * (1.0f + erff(pw1_out[i] * 0.7071067811865476f));

    /* pw2 (sgemm writes with beta=0, so no memset needed) */
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasNoTrans,
                cur_ch, cur_len, pw_dim, 1.0f,
                cn->pwconv2_weight, pw_dim, pw1_out, cur_len,
                0.0f, signal, cur_len);
    if (cn->pwconv2_bias)
        for (int o = 0; o < cur_ch; o++)
            for (int t = 0; t < cur_len; t++)
                signal[(int64_t)o * cur_len + t] += cn->pwconv2_bias[o];

    /* Gamma + residual */
    for (int ci = 0; ci < cur_ch; ci++) {
        float g = cn->gamma[ci];
        for (int t = 0; t < cur_len; t++)
            signal[(int64_t)ci * cur_len + t] = residual[(int64_t)ci * cur_len + t]
                + signal[(int64_t)ci * cur_len + t] * g;
    }
    return 0;
}

/* Red convolucional completa (ConvNeXt + conv inicial + bloques de upsample +
 * conv final) sobre `signal` en formato channel-first [cur_ch, cur_len]. Toma la
 * propiedad de `signal` y devuelve el audio recortado a [-1, 1]. */
static int conv_decoder_forward(qwen_tts_ctx_t *ctx, qwen_sd_workspace_t *ws,
                                 float *signal, int cur_ch, int cur_len,
                                 float **audio_out, int *n_samples_out) {
    qwen_speech_decoder_t *sd = &ctx->speech_dec;

    /* `owned` es el búfer del llamador, del que esta función toma la propiedad y que
     * libera en cuanto la primera operación lo ha consumido. Después la señal
     * vive en las ranuras A/B del espacio de trabajo y `cur` apunta a la vigente;
     * `ci` es el índice de su ranura (A=0, B=1) para alternar sin alias. */
    float *owned = signal;
    float *cur = signal;
    int ci = 0;

    if (ctx->debug) fprintf(stderr, "[DECODER] ConvNeXt upsample...\n");

    /* ConvNeXt upsample (2 blocks, 2x each → 4x total) */
    for (int b = 0; b < 2; b++) {
        qwen_sd_convnext_t *cn = &sd->convnext[b];
        if (!cn->conv_weight) { fprintf(stderr, "ERROR: ConvNeXt block %d weights missing!\n", b); free(owned); return -1; }

        /* La transpuesta va a B y la depthwise vuelve a A: la entrada de la
         * transpuesta (el búfer del llamador o A) está muerta para la depthwise. */
        int new_len = conv_transpose1d_out_len(cur_len, 2, 2);
        float *up_out = sd_ws_get(ws, QWEN_SDW_B, (size_t)cur_ch * new_len);
        if (!up_out || causal_conv_transpose1d(ws, up_out, cur, cn->conv_weight, cn->conv_bias,
                                               cur_ch, cur_ch, cur_len, new_len, 2, 2) != 0) {
            free(owned); return -1;
        }
        free(owned); owned = NULL;
        cur_len = new_len;

        /* Depthwise conv (k=7, pad=6) */
        float *dw_out = sd_ws_get(ws, QWEN_SDW_A, (size_t)cur_ch * cur_len);
        if (!dw_out) return -1;
        for (int c = 0; c < cur_ch; c++) {
            for (int t = 0; t < cur_len; t++) {
                float sum = cn->dwconv_bias ? cn->dwconv_bias[c] : 0;
                for (int k = 0; k < 7; k++) {
                    int in_pos = t - 6 + k;
                    if (in_pos >= 0 && in_pos < cur_len)
                        sum += cn->dwconv_weight[c * 7 + k] * up_out[(int64_t)c * cur_len + in_pos];
                }
                dw_out[(int64_t)c * cur_len + t] = sum;
            }
        }

        if (convnext_mlp(ws, cn, dw_out, up_out, cur_ch, cur_len) != 0) return -1;
        cur = dw_out; ci = 0;
    }

    if (ctx->debug) fprintf(stderr, "[DECODER] Initial conv...\n");

    /* Initial conv (1024→1536, k=7, pad_left=6): A -> B */
    if (!sd->initial_conv_weight) return -1;
    int new_ch = 1536;
    int new_len = conv1d_out_len(cur_len, 7, 1, 6);
    float *conv_out = sd_ws_get(ws, QWEN_SDW_B, (size_t)new_ch * new_len);
    if (!conv_out || causal_conv1d(ws, conv_out, cur, sd->initial_conv_weight, sd->initial_conv_wk,
                                   sd->initial_conv_bias,
                                   cur_ch, new_ch, cur_len, 7, 1) != 0)
        return -1;
    cur = conv_out; ci = 1; cur_ch = new_ch; cur_len = new_len;

    /* 4 Decoder upsample blocks */
    int up_rates[4] = {8, 5, 4, 3};
    int out_channels[4] = {768, 384, 192, 96};

    if (ctx->debug) fprintf(stderr, "[DECODER] Upsample blocks...\n");
    for (int b = 0; b < 4; b++) {
        qwen_sd_upsample_block_t *ub = &sd->upsample_blocks[b];
        int rate = up_rates[b];
        int kernel = rate * 2;
        int out_ch = out_channels[b];

        if (!ub->upsample.conv_weight) {
            fprintf(stderr, "ERROR: Upsample block %d weights missing!\n", b);
            return -1;
        }

        if (ub->upsample.snake_alpha && ub->upsample.snake_beta)
            snake_activation(cur, cur_ch, cur_len, ub->upsample.snake_alpha, ub->upsample.snake_beta);

        /* Transpuesta: de la ranura vigente a la otra. */
        int up_len = conv_transpose1d_out_len(cur_len, kernel, rate);
        float *up_out = sd_ws_get(ws, (qwen_sd_slot_t)(ci ^ 1), (size_t)out_ch * up_len);
        if (!up_out || causal_conv_transpose1d(ws, up_out, cur, ub->upsample.conv_weight,
                                               ub->upsample.conv_bias,
                                               cur_ch, out_ch, cur_len, up_len, kernel, rate) != 0)
            return -1;
        cur = up_out; ci ^= 1; cur_ch = out_ch; cur_len = up_len;

        /* Bloque residual: la señal vigente (ranura s) y su copia RES se conservan;
         * conv1 va a la otra ranura (o), conv2 vuelve a s y se suma con RES in situ. */
        int dilations[3] = {1, 3, 9};
        size_t n_el = (size_t)cur_ch * cur_len;
        for (int r = 0; r < 3; r++) {
            int dil = dilations[r];
            float *res = sd_ws_get(ws, QWEN_SDW_RES, n_el);
            float *o = sd_ws_get(ws, (qwen_sd_slot_t)(ci ^ 1), n_el);
            if (!res || !o) return -1;
            memcpy(res, cur, n_el * sizeof(float));

            if (ub->res_blocks[r].snake1_alpha && ub->res_blocks[r].snake1_beta)
                snake_activation(cur, cur_ch, cur_len,
                                  ub->res_blocks[r].snake1_alpha, ub->res_blocks[r].snake1_beta);

            if (causal_conv1d(ws, o, cur, ub->res_blocks[r].conv1_weight, ub->res_blocks[r].conv1_wk,
                              ub->res_blocks[r].conv1_bias,
                              cur_ch, cur_ch, cur_len, 7, dil) != 0)
                return -1;

            if (ub->res_blocks[r].snake2_alpha && ub->res_blocks[r].snake2_beta)
                snake_activation(o, cur_ch, cur_len,
                                  ub->res_blocks[r].snake2_alpha, ub->res_blocks[r].snake2_beta);

            if (causal_conv1d(ws, cur, o, ub->res_blocks[r].conv2_weight, NULL, ub->res_blocks[r].conv2_bias,
                              cur_ch, cur_ch, cur_len, 1, 1) != 0)
                return -1;

            for (int64_t i = 0; i < (int64_t)n_el; i++)
                cur[i] = res[i] + cur[i];
        }

        if (ctx->debug) fprintf(stderr, "[DECODER]   Block %d done: ch=%d, len=%d\n", b+1, cur_ch, cur_len);
    }

    if (ctx->debug) fprintf(stderr, "[DECODER] Final conv...\n");

    /* Final Snake + Conv (96→1, k=7) */
    if (!sd->final_snake.alpha || !sd->final_conv_weight) {
        fprintf(stderr, "ERROR: Final snake/conv weights missing!\n");
        return -1;
    }
    snake_activation(cur, cur_ch, cur_len, sd->final_snake.alpha, sd->final_snake.beta);

    int audio_len = conv1d_out_len(cur_len, 7, 1, 6);
    float *audio = (float *)aligned_calloc(audio_len, sizeof(float));
    if (!audio) return -1;
    for (int t = 0; t < audio_len; t++) {
        float sum = sd->final_conv_bias ? sd->final_conv_bias[0] : 0;
        for (int ic = 0; ic < cur_ch; ic++) {
            for (int k = 0; k < 7; k++) {
                int in_pos = t - (6 - k);
                if (in_pos >= 0 && in_pos < cur_len)
                    sum += sd->final_conv_weight[(int64_t)ic * 7 + k] * cur[(int64_t)ic * cur_len + in_pos];
            }
        }
        audio[t] = sum;
    }

    for (int i = 0; i < audio_len; i++) {
        if (audio[i] < -1.0f) audio[i] = -1.0f;
        if (audio[i] > 1.0f) audio[i] = 1.0f;
    }

    *audio_out = audio;
    *n_samples_out = audio_len;
    return 0;
}

/* ========================================================================
 * Red convolucional de streaming exacto
 *
 * Mismo cálculo que conv_decoder_forward, pero consume SOLO las tramas latentes
 * nuevas y arrastra el estado causal entre llamadas:
 *   - conv1d (k>1): conserva las últimas pad_left = (k-1)*dilation columnas de
 *     entrada. Una cola inicializada a cero es exactamente el relleno causal de
 *     ceros del primer fragmento.
 *   - ConvTranspose: conserva las (kernel-stride) columnas parciales de salida
 *     sin recortar y las suma por solapamiento en el fragmento siguiente (el
 *     sesgo se añade solo al emitir, de modo que el acarreo sea una suma parcial
 *     pura).
 *   - snake / LayerNorm / pointwise / GELU / conv k=1: por instante, sin estado.
 * La salida por fragmentos es por tanto igual a la decodificación completa, y
 * cada fragmento cuesta trabajo proporcional solo a sus propias tramas.
 * ======================================================================== */

/* Save the last tail_cols columns of [tail | chunk] back into tail. When the
 * chunk is shorter than the tail the window straddles both, so shift. */
static void cs_save_tail(float *tail, const float *in, int in_ch, int len, int tail_cols) {
    if (len >= tail_cols) {
        for (int ic = 0; ic < in_ch; ic++)
            memcpy(tail + (int64_t)ic * tail_cols, in + (int64_t)ic * len + (len - tail_cols),
                   (size_t)tail_cols * sizeof(float));
    } else {
        int keep = tail_cols - len;   /* columns of the old tail that survive */
        for (int ic = 0; ic < in_ch; ic++) {
            float *t = tail + (int64_t)ic * tail_cols;
            memmove(t, t + len, (size_t)keep * sizeof(float));
            memcpy(t + keep, in + (int64_t)ic * len, (size_t)len * sizeof(float));
        }
    }
}

/* Stateful causal conv1d on one chunk: prepend the saved tail, convolve, emit
 * [out_ch x len], update the tail. Returns a freshly allocated output.
 *
 * `warm` = 0 on the very first chunk of a stream, when the tail is still all
 * zeros. An all-zero prepended tail is *exactly* the causal zero padding that
 * causal_conv1d already applies on its own, so we skip building `ext` and
 * convolve the chunk directly: same output, none of the work. That work was not
 * free -- at chunk=2 the res-block conv1 (dilation 9) has tail_cols=54 against
 * len=64, i.e. ~1.8x the convolution, all of it thrown away. It is what made
 * TTFA regress ~8% on Neoverse-N1 when the exact path landed. */
static float *cs_conv1d(qwen_sd_workspace_t *ws, qwen_sd_slot_t out_slot,
                        const float *in, int in_ch, int out_ch, int len,
                        int kernel, int dilation,
                        const float *w, const float *wk, const float *b, float *tail, int warm) {
    int tail_cols = (kernel - 1) * dilation;

    if (!warm) {
        float *out = sd_ws_get(ws, out_slot, (size_t)out_ch * len);
        if (!out) return NULL;
        if (causal_conv1d(ws, out, in, w, wk, b, in_ch, out_ch, len, kernel, dilation) != 0) return NULL;
        cs_save_tail(tail, in, in_ch, len, tail_cols);
        return out;
    }

    int ext_len = tail_cols + len;
    float *ext = sd_ws_get(ws, QWEN_SDW_EXT, (size_t)in_ch * ext_len);
    if (!ext) return NULL;
    for (int ic = 0; ic < in_ch; ic++) {
        memcpy(ext + (int64_t)ic * ext_len, tail + (int64_t)ic * tail_cols,
               (size_t)tail_cols * sizeof(float));
        memcpy(ext + (int64_t)ic * ext_len + tail_cols, in + (int64_t)ic * len,
               (size_t)len * sizeof(float));
    }
    /* New tail = last tail_cols columns of [tail | chunk]. When len < tail_cols
     * the window still straddles the old tail, so copy from ext (not from in). */
    for (int ic = 0; ic < in_ch; ic++)
        memcpy(tail + (int64_t)ic * tail_cols, ext + (int64_t)ic * ext_len + len,
               (size_t)tail_cols * sizeof(float));

    float *full = sd_ws_get(ws, QWEN_SDW_FULL, (size_t)out_ch * ext_len);
    if (!full) return NULL;
    if (causal_conv1d(ws, full, ext, w, wk, b, in_ch, out_ch, ext_len, kernel, dilation) != 0) return NULL;

    /* Drop the first tail_cols outputs: those re-pad with zeros (wrong) and were
     * already emitted by earlier chunks. */
    float *out = sd_ws_get(ws, out_slot, (size_t)out_ch * len);
    if (!out) return NULL;
    for (int oc = 0; oc < out_ch; oc++)
        memcpy(out + (int64_t)oc * len, full + (int64_t)oc * ext_len + tail_cols,
               (size_t)len * sizeof(float));
    return out;
}

/* Stateful causal ConvTranspose1d on one chunk: run untrimmed, overlap-add the
 * carry into the head, emit len*stride columns (+bias), save the new
 * (kernel-stride)-column carry (bias-free partial sums).
 * carry == NULL is allowed when kernel == stride (no cross-chunk overlap). */
static float *cs_convt(qwen_sd_workspace_t *ws, qwen_sd_slot_t out_slot,
                       const float *in, int in_ch, int out_ch, int len,
                       int kernel, int stride,
                       const float *w, const float *b, float *carry) {
    int cs = kernel - stride;
    int full_len = (len - 1) * stride + kernel;   /* = len*stride + cs */
    int out_len = len * stride;
    float *full = sd_ws_get(ws, QWEN_SDW_FULL, (size_t)out_ch * full_len);
    if (!full) return NULL;
    /* out_len = full_len here, so causal_conv_transpose1d writes the untrimmed
     * output including the cs tail columns we need as the carry. Bias is NULL:
     * it must not be baked into the carry (it is added once, on emit). La
     * transpuesta pone a cero `full` por sí misma antes de acumular. */
    if (causal_conv_transpose1d(ws, full, in, w, NULL, in_ch, out_ch, len, full_len,
                                kernel, stride) != 0)
        return NULL;
    float *out = sd_ws_get(ws, out_slot, (size_t)out_ch * out_len);
    if (!out) return NULL;
    for (int oc = 0; oc < out_ch; oc++) {
        float *f = full + (int64_t)oc * full_len;
        float *o = out + (int64_t)oc * out_len;
        if (carry) {
            const float *cr = carry + (int64_t)oc * cs;
            for (int i = 0; i < cs; i++) f[i] += cr[i];
        }
        float bb = b ? b[oc] : 0.0f;
        for (int t = 0; t < out_len; t++) o[t] = f[t] + bb;
        if (carry) {
            float *cr = carry + (int64_t)oc * cs;
            for (int i = 0; i < cs; i++) cr[i] = f[out_len + i];
        }
    }
    return out;
}

/* Stateful ConvNeXt depthwise conv (k=7, pad_left=6) with a 6-column tail. */
static float *cs_dwconv(qwen_sd_workspace_t *ws, qwen_sd_slot_t out_slot,
                        const float *in, int ch, int len,
                        const float *w, const float *b, float *tail) {
    float *out = sd_ws_get(ws, out_slot, (size_t)ch * len);
    if (!out) return NULL;
    for (int c = 0; c < ch; c++) {
        const float *src = in + (int64_t)c * len;
        float *tl = tail + (int64_t)c * 6;
        float *dst = out + (int64_t)c * len;
        float bb = b ? b[c] : 0.0f;
        const float *wc = w + (int64_t)c * 7;
        for (int t = 0; t < len; t++) {
            float sum = bb;
            for (int k = 0; k < 7; k++) {
                int p = t - 6 + k;                  /* chunk-relative input pos */
                sum += wc[k] * (p >= 0 ? src[p] : tl[6 + p]);
            }
            dst[t] = sum;
        }
        /* New tail = last 6 inputs of [tail | chunk] */
        if (len >= 6) memcpy(tl, src + len - 6, 6 * sizeof(float));
        else
            for (int i = 0; i < 6; i++)
                tl[i] = (i + len < 6) ? tl[i + len] : src[i + len - 6];
    }
    return out;
}

/* Lazily allocate (zeroed) per-slot streaming conv state. */
static int cs_ensure_alloc(qwen_sd_stream_state_t *st) {
    if (st->cs_alloc) return 0;
    static const int up_out_ch[4] = {768, 384, 192, 96};
    static const int up_rate[4]   = {8, 5, 4, 3};
    static const int dils[3]      = {1, 3, 9};
    st->cs_cn_dw_tail[0] = (float *)aligned_calloc(1024 * 6, sizeof(float));
    st->cs_cn_dw_tail[1] = (float *)aligned_calloc(1024 * 6, sizeof(float));
    st->cs_init_tail     = (float *)aligned_calloc(1024 * 6, sizeof(float));
    st->cs_final_tail    = (float *)aligned_calloc(96 * 6, sizeof(float));
    if (!st->cs_cn_dw_tail[0] || !st->cs_cn_dw_tail[1] || !st->cs_init_tail || !st->cs_final_tail)
        return -1;
    for (int b = 0; b < 4; b++) {
        st->cs_up_carry[b] = (float *)aligned_calloc((int64_t)up_out_ch[b] * up_rate[b],
                                                     sizeof(float));
        if (!st->cs_up_carry[b]) return -1;
        for (int r = 0; r < 3; r++) {
            st->cs_res_tail[b][r] = (float *)aligned_calloc((int64_t)up_out_ch[b] * 6 * dils[r],
                                                            sizeof(float));
            if (!st->cs_res_tail[b][r]) return -1;
        }
    }
    st->cs_alloc = 1;
    return 0;
}

/* Unidades residuales por tramos de tiempo en el decodificador en streaming.
 * En los bloques de sobremuestreo finales, cada una de las 3 unidades residuales
 * recorre varias veces la señal entera del bloque (copia, Snake, convolución
 * dilatada, Snake, convolución k=1 y suma), y esa señal ocupa varios MB que no
 * caben en la caché L2: el tráfico a memoria resultante compite con el Talker y el
 * Code Predictor. Procesar las 3 unidades sobre tramos de SD_TILE_BYTES por búfer
 * deja los datos de cada tramo en la L2. Con 256 KB el tramo cabe en la L2 y, a la
 * vez, es lo bastante largo para que las columnas de cola que cada convolución
 * dilatada vuelve a calcular por tramo no pesen; con tramos menores ese recálculo
 * domina. Se aplica desde el bloque SD_TILE_FROM_BLOCK (up2), porque las dos
 * últimas etapas, con la señal más larga, concentran el tráfico.
 * Las colas con estado de cada convolución dilatada (cs_res_tail) encadenan los
 * tramos igual que encadenan los fragmentos del flujo: la cola avanza tramo a
 * tramo, y solo el primer tramo del primer fragmento arranca sin contexto.
 * Las ranuras QWEN_SDW_PW1 y QWEN_SDW_CT están libres en este punto (cs_conv1d usa
 * QWEN_SDW_EXT, QWEN_SDW_FULL y QWEN_SDW_XP), así que guardan el tramo y su copia
 * residual; free_slot es la ranura alterna de la señal vigente `cur`. */
enum { SD_TILE_BYTES = 256 * 1024, SD_TILE_FROM_BLOCK = 2 };

static int cs_res_tiled(qwen_sd_workspace_t *ws, qwen_sd_stream_state_t *st,
                        qwen_sd_upsample_block_t *ub, int b, float *cur, int cur_ch, int cur_len,
                        qwen_sd_slot_t free_slot) {
    static const int dilations[3] = {1, 3, 9};
    int T = SD_TILE_BYTES / (4 * cur_ch);
    if (T < 16) T = 16;
    for (int t0 = 0, ti = 0; t0 < cur_len; t0 += T, ti++) {
        int n = cur_len - t0 < T ? cur_len - t0 : T;
        float *tin = sd_ws_get(ws, QWEN_SDW_PW1, (size_t)cur_ch * n);
        float *res = sd_ws_get(ws, QWEN_SDW_CT, (size_t)cur_ch * n);
        if (!tin || !res) return -1;
        for (int c = 0; c < cur_ch; c++)
            memcpy(tin + (size_t)c * n, cur + (size_t)c * cur_len + t0, (size_t)n * sizeof(float));
        int warm = st->cs_warm || ti > 0;
        for (int r = 0; r < 3; r++) {
            memcpy(res, tin, (size_t)cur_ch * n * sizeof(float));
            if (ub->res_blocks[r].snake1_alpha && ub->res_blocks[r].snake1_beta)
                snake_activation(tin, cur_ch, n, ub->res_blocks[r].snake1_alpha, ub->res_blocks[r].snake1_beta);
            float *c1 = cs_conv1d(ws, free_slot, tin, cur_ch, cur_ch, n, 7, dilations[r],
                                  ub->res_blocks[r].conv1_weight, ub->res_blocks[r].conv1_wk,
                                  ub->res_blocks[r].conv1_bias, st->cs_res_tail[b][r], warm);
            if (!c1) return -1;
            if (ub->res_blocks[r].snake2_alpha && ub->res_blocks[r].snake2_beta)
                snake_activation(c1, cur_ch, n, ub->res_blocks[r].snake2_alpha, ub->res_blocks[r].snake2_beta);
            if (causal_conv1d(ws, tin, c1, ub->res_blocks[r].conv2_weight, NULL, ub->res_blocks[r].conv2_bias,
                              cur_ch, cur_ch, n, 1, 1) != 0)
                return -1;
            for (int64_t i = 0; i < (int64_t)cur_ch * n; i++) tin[i] = res[i] + tin[i];
        }
        for (int c = 0; c < cur_ch; c++)
            memcpy(cur + (size_t)c * cur_len + t0, tin + (size_t)c * n, (size_t)n * sizeof(float));
    }
    return 0;
}

/* Streaming conv decoder: consumes `signal` [1024 x m] (takes ownership),
 * emits exactly m*1920 samples. Mirrors conv_decoder_forward stage by stage. */
static int conv_decoder_forward_streaming(qwen_tts_ctx_t *ctx, qwen_sd_stream_state_t *st,
                                          float *signal, int m,
                                          float **audio_out, int *n_samples_out) {
    qwen_speech_decoder_t *sd = &ctx->speech_dec;
    qwen_sd_workspace_t *ws = &ctx->sd_ws;
    if (cs_ensure_alloc(st) != 0) { free(signal); return -1; }
    int cur_ch = 1024, cur_len = m;

    /* `owned` es el búfer del llamador (se libera tras la primera operación);
     * después la señal vive en las ranuras A/B del espacio de trabajo: `cur` es la
     * vigente y `ci` su índice (A=0, B=1) para alternar sin alias. */
    float *owned = signal;
    float *cur = signal;
    int ci = 0;

    /* ConvNeXt upsample (2 blocks, 2x each). k=2,s=2 → no overlap, carry-free. */
    for (int b = 0; b < 2; b++) {
        qwen_sd_convnext_t *cn = &sd->convnext[b];
        if (!cn->conv_weight) { free(owned); return -1; }
        /* La transpuesta va a B y la depthwise vuelve a A (su entrada ya está muerta). */
        float *up = cs_convt(ws, QWEN_SDW_B, cur, cur_ch, cur_ch, cur_len, 2, 2,
                             cn->conv_weight, cn->conv_bias, NULL);
        free(owned); owned = NULL;
        if (!up) return -1;
        cur_len *= 2;
        float *dw = cs_dwconv(ws, QWEN_SDW_A, up, cur_ch, cur_len, cn->dwconv_weight, cn->dwconv_bias,
                              st->cs_cn_dw_tail[b]);
        if (!dw) return -1;
        if (convnext_mlp(ws, cn, dw, up, cur_ch, cur_len) != 0) return -1;
        cur = dw; ci = 0;
    }

    /* Initial conv (1024→1536, k=7): A -> B */
    if (!sd->initial_conv_weight) return -1;
    float *ic_out = cs_conv1d(ws, QWEN_SDW_B, cur, cur_ch, 1536, cur_len, 7, 1,
                              sd->initial_conv_weight, sd->initial_conv_wk, sd->initial_conv_bias,
                              st->cs_init_tail, st->cs_warm);
    if (!ic_out) return -1;
    cur = ic_out; ci = 1; cur_ch = 1536;

    /* 4 Decoder upsample blocks */
    int up_rates[4] = {8, 5, 4, 3};
    int out_channels[4] = {768, 384, 192, 96};

    for (int b = 0; b < 4; b++) {
        qwen_sd_upsample_block_t *ub = &sd->upsample_blocks[b];
        int rate = up_rates[b];
        int kernel = rate * 2;
        int out_ch = out_channels[b];
        if (!ub->upsample.conv_weight) return -1;

        if (ub->upsample.snake_alpha && ub->upsample.snake_beta)
            snake_activation(cur, cur_ch, cur_len, ub->upsample.snake_alpha, ub->upsample.snake_beta);

        float *up_out = cs_convt(ws, (qwen_sd_slot_t)(ci ^ 1), cur, cur_ch, out_ch, cur_len, kernel, rate,
                                 ub->upsample.conv_weight, ub->upsample.conv_bias,
                                 st->cs_up_carry[b]);
        if (!up_out) return -1;
        cur = up_out; ci ^= 1; cur_ch = out_ch; cur_len *= rate;

        /* Bloque residual. Desde SD_TILE_FROM_BLOCK se procesa por tramos de tiempo
         * (ver cs_res_tiled). En los bloques anteriores, la señal vigente (ranura s) y
         * su copia RES se conservan; conv1 va a la otra ranura (o), conv2 vuelve a s
         * y se suma con RES in situ. */
        if (b >= SD_TILE_FROM_BLOCK) {
            if (cs_res_tiled(ws, st, ub, b, cur, cur_ch, cur_len, (qwen_sd_slot_t)(ci ^ 1)) != 0)
                return -1;
        } else {
            int dilations[3] = {1, 3, 9};
            size_t n_el = (size_t)cur_ch * cur_len;
            for (int r = 0; r < 3; r++) {
                int dil = dilations[r];
                float *res = sd_ws_get(ws, QWEN_SDW_RES, n_el);
                if (!res) return -1;
                memcpy(res, cur, n_el * sizeof(float));

                if (ub->res_blocks[r].snake1_alpha && ub->res_blocks[r].snake1_beta)
                    snake_activation(cur, cur_ch, cur_len,
                                     ub->res_blocks[r].snake1_alpha, ub->res_blocks[r].snake1_beta);

                float *c1_out = cs_conv1d(ws, (qwen_sd_slot_t)(ci ^ 1), cur, cur_ch, cur_ch, cur_len, 7, dil,
                                          ub->res_blocks[r].conv1_weight, ub->res_blocks[r].conv1_wk,
                                          ub->res_blocks[r].conv1_bias,
                                          st->cs_res_tail[b][r], st->cs_warm);
                if (!c1_out) return -1;

                if (ub->res_blocks[r].snake2_alpha && ub->res_blocks[r].snake2_beta)
                    snake_activation(c1_out, cur_ch, cur_len,
                                     ub->res_blocks[r].snake2_alpha, ub->res_blocks[r].snake2_beta);

                /* conv2 is k=1: stateless */
                if (causal_conv1d(ws, cur, c1_out, ub->res_blocks[r].conv2_weight, NULL, ub->res_blocks[r].conv2_bias,
                                  cur_ch, cur_ch, cur_len, 1, 1) != 0)
                    return -1;

                for (int64_t i = 0; i < (int64_t)n_el; i++)
                    cur[i] = res[i] + cur[i];
            }
        }
    }

    /* Final Snake + Conv (96→1, k=7) with a 6-column input tail */
    if (!sd->final_snake.alpha || !sd->final_conv_weight) return -1;
    snake_activation(cur, cur_ch, cur_len, sd->final_snake.alpha, sd->final_snake.beta);

    float *audio = (float *)aligned_calloc(cur_len, sizeof(float));
    if (!audio) return -1;
    {
        float *tl = st->cs_final_tail;   /* [96 x 6] */
        if (sd->final_conv_bias)
            for (int t = 0; t < cur_len; t++) audio[t] = sd->final_conv_bias[0];
        for (int ic = 0; ic < cur_ch; ic++) {
            const float *src = cur + (int64_t)ic * cur_len;
            const float *wc  = sd->final_conv_weight + (int64_t)ic * 7;
            const float *tc  = tl + (int64_t)ic * 6;
            for (int t = 0; t < cur_len; t++) {
                float sum = 0;
                for (int k = 0; k < 7; k++) {
                    int p = t - 6 + k;
                    sum += wc[k] * (p >= 0 ? src[p] : tc[6 + p]);
                }
                audio[t] += sum;
            }
        }
        /* Update the tail with the last 6 (snake-activated) inputs */
        for (int ic = 0; ic < cur_ch; ic++) {
            float *tc = tl + (int64_t)ic * 6;
            const float *src = cur + (int64_t)ic * cur_len;
            if (cur_len >= 6) memcpy(tc, src + cur_len - 6, 6 * sizeof(float));
            else
                for (int i = 0; i < 6; i++)
                    tc[i] = (i + cur_len < 6) ? tc[i + cur_len] : src[i + cur_len - 6];
        }
    }

    for (int i = 0; i < cur_len; i++) {
        if (audio[i] < -1.0f) audio[i] = -1.0f;
        if (audio[i] > 1.0f) audio[i] = 1.0f;
    }

    st->cs_warm = 1;   /* tails now hold real context; later chunks must prepend them */

    *audio_out = audio;
    *n_samples_out = cur_len;
    return 0;
}

/* Decodificación incremental en streaming: procesa solo new_frames por
 * VQ→pre-transformer (con caché KV), guarda la salida latente y ejecuta la red
 * convolucional con estado sobre las tramas nuevas. Devuelve solo las muestras
 * NUEVAS (no las emitidas antes). */
/* Explicit-state variant: streams using the caller-owned `st` instead of the
 * single ctx->sd_stream. Lets the continuous-batching driver keep B independent
 * per-slot streaming decoder states (weights in ctx->speech_dec + ctx->config are
 * read-only, shared safely). */
int qwen_speech_decoder_decode_streaming_st(qwen_tts_ctx_t *ctx, qwen_sd_stream_state_t *st,
                                          const int *new_codes, int new_frames,
                                          float **audio_out, int *n_samples) {
    qwen_speech_decoder_t *sd = &ctx->speech_dec;
    qwen_tts_config_t *c = &ctx->config;

    int cb_dim = QWEN_TTS_CODEBOOK_DIM;
    int vq_hidden = 512;
    int latent_dim = 1024;
    int dec_hidden = 512;
    int dec_inter = 1024;
    int n_heads = 16;
    int head_dim = c->dec_head_dim;
    int qkv_dim = n_heads * head_dim;
    int window = 72;
    float eps = c->dec_rms_norm_eps;
    int half_hd = head_dim / 2;

    /* === Step 1: VQ dequant for new frames only === */
    /* Output: vq_out row-major [new_frames, 512] */
    float *vq_out = (float *)aligned_calloc((int64_t)new_frames * vq_hidden, sizeof(float));
    float *cb_sum = (float *)aligned_malloc(cb_dim * sizeof(float));

    for (int f = 0; f < new_frames; f++) {
        int code0 = new_codes[f * 16];
        if (code0 >= 0 && code0 < c->codebook_size && sd->codebook[0]) {
            const float *emb = sd->codebook[0] + (int64_t)code0 * cb_dim;
            if (sd->rvq_first_output_proj) {
                for (int o = 0; o < vq_hidden; o++) {
                    float sum = 0;
                    for (int i = 0; i < cb_dim; i++)
                        sum += sd->rvq_first_output_proj[(int64_t)o * cb_dim + i] * emb[i];
                    vq_out[(int64_t)f * vq_hidden + o] += sum;
                }
            }
        }
        memset(cb_sum, 0, cb_dim * sizeof(float));
        for (int k = 1; k < 16; k++) {
            int code = new_codes[f * 16 + k];
            if (code >= 0 && code < c->codebook_size && sd->codebook[k]) {
                const float *emb = sd->codebook[k] + (int64_t)code * cb_dim;
                for (int d = 0; d < cb_dim; d++) cb_sum[d] += emb[d];
            }
        }
        if (sd->rvq_rest_output_proj) {
            for (int o = 0; o < vq_hidden; o++) {
                float sum = 0;
                for (int i = 0; i < cb_dim; i++)
                    sum += sd->rvq_rest_output_proj[(int64_t)o * cb_dim + i] * cb_sum[i];
                vq_out[(int64_t)f * vq_hidden + o] += sum;
            }
        }
    }
    free(cb_sum);

    /* === Step 2: Pre-conv on new frames with padding from previous chunk === */
    /* VQ output is row-major [new_frames, 512]. Transpose to channel-first [512, new_frames]
     * for conv1d, prepending 2 frames of padding. */
    int pad_frames = st->vq_pad_valid ? 2 : 0;
    int conv_in_len = pad_frames + new_frames;
    float *vq_cf = (float *)aligned_calloc((int64_t)vq_hidden * conv_in_len, sizeof(float));

    /* Copy padding */
    if (st->vq_pad_valid && st->vq_pad) {
        for (int ch = 0; ch < vq_hidden; ch++)
            for (int t = 0; t < 2; t++)
                vq_cf[(int64_t)ch * conv_in_len + t] = st->vq_pad[(int64_t)ch * 2 + t];
    }
    /* Copy new VQ output (transpose row→channel-first) */
    for (int f = 0; f < new_frames; f++)
        for (int ch = 0; ch < vq_hidden; ch++)
            vq_cf[(int64_t)ch * conv_in_len + pad_frames + f] = vq_out[(int64_t)f * vq_hidden + ch];

    /* Save last 2 frames of VQ output (channel-first) as padding for next chunk */
    if (!st->vq_pad) st->vq_pad = (float *)aligned_malloc(vq_hidden * 2 * sizeof(float));
    int save_start = (conv_in_len >= 2) ? conv_in_len - 2 : 0;
    int save_count = (conv_in_len >= 2) ? 2 : conv_in_len;
    for (int ch = 0; ch < vq_hidden; ch++)
        for (int t = 0; t < save_count; t++)
            st->vq_pad[(int64_t)ch * 2 + (2 - save_count + t)] =
                vq_cf[(int64_t)ch * conv_in_len + save_start + t];
    if (save_count < 2) {
        /* Zero-fill the earlier positions if we had fewer than 2 frames */
        for (int ch = 0; ch < vq_hidden; ch++)
            for (int t = 0; t < 2 - save_count; t++)
                st->vq_pad[(int64_t)ch * 2 + t] = 0;
    }
    st->vq_pad_valid = 1;
    free(vq_out);

    /* Pre-conv (512→1024, k=3, causal, pad_left=2) */
    float *pre_conv_out = (float *)aligned_calloc((int64_t)latent_dim * conv_in_len, sizeof(float));
    if (causal_conv1d(&ctx->sd_ws, pre_conv_out, vq_cf, sd->pre_conv_weight, sd->pre_conv_wk, sd->pre_conv_bias,
                      vq_hidden, latent_dim, conv_in_len, 3, 1) != 0) {
        free(pre_conv_out); free(vq_cf);
        return -1;
    }
    free(vq_cf);

    /* Take only the last new_frames from pre_conv output */
    /* The first pad_frames outputs may have been computed with actual previous context */

    /* === Step 3: Input proj on new frames (1024→512, row-major) === */
    float *hidden = (float *)aligned_malloc((int64_t)new_frames * dec_hidden * sizeof(float));
    /* Transpose new portion [1024, new_frames] → [new_frames, 1024] */
    float *pre_conv_rm = (float *)aligned_malloc((int64_t)new_frames * latent_dim * sizeof(float));
    for (int f = 0; f < new_frames; f++)
        for (int d = 0; d < latent_dim; d++)
            pre_conv_rm[(int64_t)f * latent_dim + d] = pre_conv_out[(int64_t)d * conv_in_len + pad_frames + f];
    free(pre_conv_out);
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                new_frames, dec_hidden, latent_dim, 1.0f,
                pre_conv_rm, latent_dim,
                sd->input_proj_weight, latent_dim,
                0.0f, hidden, dec_hidden);
    free(pre_conv_rm);
    if (sd->input_proj_bias)
        for (int f = 0; f < new_frames; f++)
            for (int o = 0; o < dec_hidden; o++)
                hidden[(int64_t)f * dec_hidden + o] += sd->input_proj_bias[o];

    /* === Step 4: Pre-transformer with KV cache === */
    /* Acota la caché KV a O(window+chunk) en vez del stream completo.
     * Tramas físicas necesarias = (kv_len + new_frames) - kv_base. Cuando eso supera
     * la reserva, primero se COMPACTA: las próximas consultas solo leen hacia atrás hasta
     * kv_len-window+1, así que se descarta todo lo anterior y se mueve con memmove la cola viva (<window
     * tramas) a la posición física 0, reajustando kv_base. Solo si un único bloque
     * supera el tope se crece (raro). índice físico = abs - kv_base. */
    int need = (st->kv_len + new_frames) - st->kv_base;
    if (need > st->kv_alloc) {
        int keep_from = st->kv_len - (window - 1);
        if (keep_from < 0) keep_from = 0;
        if (keep_from > st->kv_base && st->kv_len > st->kv_base) {
            int keep = st->kv_len - keep_from;          /* live tail, < window */
            int shift = keep_from - st->kv_base;        /* physical frames dropped */
            for (int l = 0; l < c->dec_num_layers; l++) {
                if (keep > 0) {
                    memmove(st->k_cache[l], st->k_cache[l] + (int64_t)shift * qkv_dim,
                            (int64_t)keep * qkv_dim * sizeof(float));
                    memmove(st->v_cache[l], st->v_cache[l] + (int64_t)shift * qkv_dim,
                            (int64_t)keep * qkv_dim * sizeof(float));
                }
            }
            st->kv_base = keep_from;
            need = (st->kv_len + new_frames) - st->kv_base;
        }
        if (need > st->kv_alloc) {                        /* chunk bigger than cap */
            int new_alloc = need + 256;
            for (int l = 0; l < c->dec_num_layers; l++) {
                st->k_cache[l] = (float *)realloc(st->k_cache[l], (int64_t)new_alloc * qkv_dim * sizeof(float));
                st->v_cache[l] = (float *)realloc(st->v_cache[l], (int64_t)new_alloc * qkv_dim * sizeof(float));
            }
            st->kv_alloc = new_alloc;
        }
    }

    float *q = (float *)aligned_malloc((int64_t)new_frames * qkv_dim * sizeof(float));
    float *new_k = (float *)aligned_malloc((int64_t)new_frames * qkv_dim * sizeof(float));
    float *new_v = (float *)aligned_malloc((int64_t)new_frames * qkv_dim * sizeof(float));
    float *x_norm = (float *)aligned_malloc((int64_t)new_frames * dec_hidden * sizeof(float));
    float *attn_out = (float *)aligned_malloc((int64_t)new_frames * qkv_dim * sizeof(float));

    for (int layer = 0; layer < c->dec_num_layers; layer++) {
        qwen_sd_pre_layer_t *l = &sd->pre_layers[layer];

        /* Input RMSNorm (NEON-optimized) */
        qwen_rms_norm(x_norm, hidden, l->attn_norm, new_frames, dec_hidden, eps);

        /* QKV projections for new frames */
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    new_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_q, dec_hidden, 0.0f, q, qkv_dim);
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    new_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_k, dec_hidden, 0.0f, new_k, qkv_dim);
        cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                    new_frames, qkv_dim, dec_hidden, 1.0f,
                    x_norm, dec_hidden, l->attn_v, dec_hidden, 0.0f, new_v, qkv_dim);

        /* NeoX split-half RoPE using absolute positions */
        for (int s = 0; s < new_frames; s++) {
            int abs_pos = st->kv_len + s;
            const float *cos_ptr = sd->rope_cos + abs_pos * half_hd;
            const float *sin_ptr = sd->rope_sin + abs_pos * half_hd;
            for (int h = 0; h < n_heads; h++) {
                float *qh = q + s * qkv_dim + h * head_dim;
                float *kh = new_k + s * qkv_dim + h * head_dim;
                for (int i = 0; i < half_hd; i++) {
                    float q1 = qh[i], q2 = qh[i + half_hd];
                    float k1 = kh[i], k2 = kh[i + half_hd];
                    float co = cos_ptr[i], si = sin_ptr[i];
                    qh[i]           = q1 * co - q2 * si;
                    qh[i + half_hd] = q2 * co + q1 * si;
                    kh[i]           = k1 * co - k2 * si;
                    kh[i + half_hd] = k2 * co + k1 * si;
                }
            }
        }

        /* Append new K, V to cache for this layer (physical = abs - kv_base) */
        memcpy(st->k_cache[layer] + (int64_t)(st->kv_len - st->kv_base) * qkv_dim,
               new_k, (int64_t)new_frames * qkv_dim * sizeof(float));
        memcpy(st->v_cache[layer] + (int64_t)(st->kv_len - st->kv_base) * qkv_dim,
               new_v, (int64_t)new_frames * qkv_dim * sizeof(float));

        /* Sliding window causal attention: Q from new frames, K/V from cache */
        float scale = 1.0f / sqrtf((float)head_dim);
        for (int sq = 0; sq < new_frames; sq++) {
            int abs_sq = st->kv_len + sq;
            float *out = attn_out + sq * qkv_dim;
            memset(out, 0, qkv_dim * sizeof(float));
            int sk_start = (abs_sq - window + 1 > 0) ? abs_sq - window + 1 : 0;
            int sk_end = abs_sq; /* inclusive */

            for (int h = 0; h < n_heads; h++) {
                const float *qh = q + sq * qkv_dim + h * head_dim;
                float *oh = out + h * head_dim;

                int n_keys = sk_end - sk_start + 1;
                float scores_buf[512];
                float *scores = n_keys <= 512 ? scores_buf : (float *)malloc(n_keys * sizeof(float));
                float max_score = -1e30f;
                for (int j = 0; j < n_keys; j++) {
                    int sk = sk_start + j;
                    const float *kh = st->k_cache[layer] + (int64_t)(sk - st->kv_base) * qkv_dim + h * head_dim;
                    float dot = 0;
                    for (int d = 0; d < head_dim; d++) dot += qh[d] * kh[d];
                    scores[j] = dot * scale;
                    if (scores[j] > max_score) max_score = scores[j];
                }

                float sum_exp = 0;
                for (int j = 0; j < n_keys; j++) {
                    scores[j] = expf(scores[j] - max_score);
                    sum_exp += scores[j];
                }
                float inv_sum = 1.0f / sum_exp;

                for (int j = 0; j < n_keys; j++) {
                    int sk = sk_start + j;
                    const float *vh = st->v_cache[layer] + (int64_t)(sk - st->kv_base) * qkv_dim + h * head_dim;
                    float w = scores[j] * inv_sum;
                    for (int d = 0; d < head_dim; d++) oh[d] += vh[d] * w;
                }
                if (scores != scores_buf) free(scores);
            }
        }

        /* Output proj + layer_scale + residual */
        {
            float *oproj = x_norm;
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        new_frames, dec_hidden, qkv_dim, 1.0f,
                        attn_out, qkv_dim, l->attn_o, qkv_dim,
                        0.0f, oproj, dec_hidden);
            for (int s = 0; s < new_frames; s++) {
                float *xs = hidden + s * dec_hidden;
                float *ps = oproj + s * dec_hidden;
                if (l->attn_layer_scale) {
                    for (int o = 0; o < dec_hidden; o++) xs[o] += ps[o] * l->attn_layer_scale[o];
                } else {
                    for (int o = 0; o < dec_hidden; o++) xs[o] += ps[o];
                }
            }
        }

        /* Post-attn RMSNorm (NEON-optimized) */
        qwen_rms_norm(x_norm, hidden, l->ffn_norm, new_frames, dec_hidden, eps);

        /* SwiGLU FFN */
        {
            float *ffn_gate = (float *)aligned_malloc((int64_t)new_frames * dec_inter * sizeof(float));
            float *ffn_up = (float *)aligned_malloc((int64_t)new_frames * dec_inter * sizeof(float));
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        new_frames, dec_inter, dec_hidden, 1.0f,
                        x_norm, dec_hidden, l->ffn_gate, dec_hidden,
                        0.0f, ffn_gate, dec_inter);
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        new_frames, dec_inter, dec_hidden, 1.0f,
                        x_norm, dec_hidden, l->ffn_up, dec_hidden,
                        0.0f, ffn_up, dec_inter);
            for (int64_t i = 0; i < (int64_t)new_frames * dec_inter; i++)
                ffn_gate[i] = (ffn_gate[i] / (1.0f + expf(-ffn_gate[i]))) * ffn_up[i];
            free(ffn_up);
            float *ffn_down_out = (float *)aligned_malloc((int64_t)new_frames * dec_hidden * sizeof(float));
            cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                        new_frames, dec_hidden, dec_inter, 1.0f,
                        ffn_gate, dec_inter, l->ffn_down, dec_inter,
                        0.0f, ffn_down_out, dec_hidden);
            free(ffn_gate);
            for (int s = 0; s < new_frames; s++) {
                float *hs = hidden + s * dec_hidden;
                float *ds = ffn_down_out + s * dec_hidden;
                if (l->ffn_layer_scale) {
                    for (int o = 0; o < dec_hidden; o++) hs[o] += ds[o] * l->ffn_layer_scale[o];
                } else {
                    for (int o = 0; o < dec_hidden; o++) hs[o] += ds[o];
                }
            }
            free(ffn_down_out);
        }
    }

    /* Update KV cache length (after all layers processed) */
    st->kv_len += new_frames;

    free(q); free(new_k); free(new_v); free(x_norm); free(attn_out);

    /* === Step 5: Final RMSNorm + Output proj (512→1024) on new frames === */
    if (sd->final_norm_weight) {
        qwen_rms_norm(hidden, hidden, sd->final_norm_weight, new_frames, dec_hidden, eps);
    }

    /* Caché latente: el decodificador convolucional (Paso 6) solo lee las tramas
     * nuevas del fragmento en curso, así que al agotarse la capacidad se descarta
     * todo lo anterior en vez de conservar el flujo completo. Índice físico =
     * absoluto - latent_base. */
    int lat_need = (st->latent_frames + new_frames) - st->latent_base;
    if (lat_need > st->latent_alloc) {
        if (st->latent_frames > st->latent_base) {
            st->latent_base = st->latent_frames;
            lat_need = new_frames;
        }
        if (lat_need > st->latent_alloc) {
            int new_alloc = lat_need + 256;
            st->latent_cache = (float *)realloc(st->latent_cache,
                (int64_t)new_alloc * latent_dim * sizeof(float));
            st->latent_alloc = new_alloc;
        }
    }

    /* Output proj new frames → append to latent cache [row-major: frames × 1024] */
    float *lat_dst = st->latent_cache + (int64_t)(st->latent_frames - st->latent_base) * latent_dim;
    cblas_sgemm(CblasRowMajor, CblasNoTrans, CblasTrans,
                new_frames, latent_dim, dec_hidden, 1.0f,
                hidden, dec_hidden,
                sd->output_proj_weight, dec_hidden,
                0.0f, lat_dst, latent_dim);
    if (sd->output_proj_bias)
        for (int f = 0; f < new_frames; f++)
            for (int o = 0; o < latent_dim; o++)
                lat_dst[(int64_t)f * latent_dim + o] += sd->output_proj_bias[o];
    st->latent_frames += new_frames;
    free(hidden);

    /* === Step 6: conv decoder === */
    /* Se alimenta solo con las tramas nuevas: las colas y acarreos de cada
     * convolución guardados en `st` contienen todo lo que la pila causal necesita
     * del pasado. Emite exactamente new_frames*1920 muestras, iguales a la porción
     * correspondiente de una decodificación completa. El índice físico se rebasa
     * con latent_base porque la caché latente se descarta al agotarse. */
    float *signal = (float *)aligned_malloc((int64_t)latent_dim * new_frames * sizeof(float));
    if (!signal) return -1;
    const float *lat_new = st->latent_cache
        + (int64_t)(st->latent_frames - new_frames - st->latent_base) * latent_dim;
    for (int f = 0; f < new_frames; f++)
        for (int d = 0; d < latent_dim; d++)
            signal[(int64_t)d * new_frames + f] = lat_new[(int64_t)f * latent_dim + d];

    float *new_audio = NULL;
    int new_samples = 0;
    int ret = conv_decoder_forward_streaming(ctx, st, signal, new_frames,
                                             &new_audio, &new_samples);
    if (ret != 0) return ret;

    st->frames_decoded += new_frames;
    st->samples_produced += new_samples;
    *audio_out = new_audio;
    *n_samples = new_samples;
    return 0;
}

/* Back-compat wrapper: stream using the single ctx->sd_stream state. */
int qwen_speech_decoder_decode_streaming(qwen_tts_ctx_t *ctx,
                                          const int *new_codes, int new_frames,
                                          float **audio_out, int *n_samples) {
    return qwen_speech_decoder_decode_streaming_st(ctx, &ctx->sd_stream,
                                                   new_codes, new_frames, audio_out, n_samples);
}
