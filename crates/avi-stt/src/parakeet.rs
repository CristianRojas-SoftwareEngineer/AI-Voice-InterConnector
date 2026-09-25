//! Motor STT real sobre Parakeet TDT 0.6B v3 int8 vía `ort`/ONNX Runtime.
//!
//! Pipeline portado del spike validado en disco
//! (`%TEMP%\opencode\spike-parakeet\src\main.rs`):
//! `nemo128.onnx` (extracción de features) → `encoder-model.int8.onnx`
//! → `decoder_joint-model.int8.onnx` + decodificador **TDT greedy**.
//!
//! El motor implementa `avi_core::engine::SttEngine` y transcribe PCM `i16`
//! mono a 16 kHz. Whisper solo transcribe, nunca traduce; Parakeet tampoco
//! traduce (la traducción vía Marian/ct2rs vive en `avi-translation`, aislada).
//!
//! ## Peculiaridades del export int8 de `istupakov/parakeet-tdt-0.6b-v3-onnx`
//!
//! - `encoder_outputs` se consume con layout `[B, DIM=1024, T']` **sin
//!   transponer** (el export fp32 de onnx-asr sí usa transpuesto + rank-4).
//! - `targets`/`target_length` se pasan como **int32**, NO como i64.
//! - Los estados recurrentes del predictor LSTM se extraen por nombre
//!   `output_states_1` / `output_states_2` (hay un `prednet_lengths` intermedio
//!   que se ignora).

use std::path::Path;

use anyhow::Context;
use ort::inputs;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;

use avi_core::engine::{available_threads, SttEngine};

/// Dimensiones de la red de predicción del FastConformer-TDT 0.6B (capas LSTM × oculto).
const PRED_LAYERS: i64 = 2;
const PRED_HIDDEN: i64 = 640;
/// Tope de tokens emitidos por frame antes de avanzar (max_tokens_per_step de NeMo).
const MAX_TOKENS_PER_STEP: usize = 10;

/// Motor STT real sobre el export ONNX int8 de Parakeet TDT 0.6B v3.
///
/// Carga los 4 archivos canónicos del modelo (`encoder-model.int8.onnx`,
/// `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`) desde un
/// directorio, y expone `transcribe` siguiendo el trait `SttEngine`.
pub struct ParakeetEngine {
    // `ort::Session::run` requiere `&mut self` (rc.13). Como el trait
    // `SttEngine::transcribe(&self)` es inmutable, los 3 `Session` se envuelven
    // en `Mutex` (interior mutability). La síntesis es por-request, por lo que
    // no hay paralelismo real dentro de un motor — el lock es efetivamente
    // instantáneo en el uso del daemon (el spike usaba ownership por request,
    // equivalente semántico).
    pre: std::sync::Mutex<Session>,
    enc: std::sync::Mutex<Session>,
    dj: std::sync::Mutex<Session>,
    pre_in: Vec<String>,
    enc_in: Vec<String>,
    dj_in: Vec<String>,
    vocab_size: usize,
    blank: usize,
    tokens: Vec<String>,
}

impl ParakeetEngine {
    /// Carga los 4 artefactos del modelo Parakeet desde `model_dir`.
    pub fn new(model_dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base = model_dir.as_ref();

        let pre = session(base.join("nemo128.onnx"))
            .context("fallo al cargar sesión de features nemo128")?;
        let enc = session(base.join("encoder-model.int8.onnx"))
            .context("fallo al cargar sesión del encoder int8")?;
        let dj = session(base.join("decoder_joint-model.int8.onnx"))
            .context("fallo al cargar sesión del decoder_joint int8")?;

        let pre_in = names(pre.inputs());
        let enc_in = names(enc.inputs());
        let dj_in = names(dj.inputs());

        // Vocabulario: líneas "token índice". Los tokens usan el marcador de
        // sub-palabra `▁` (espacio) de SentencePiece; se pliega a espacio.
        let mut tokens = Vec::new();
        let vocab =
            std::fs::read_to_string(base.join("vocab.txt")).context("fallo al leer vocab.txt")?;
        for linea in vocab.lines() {
            if linea.trim().is_empty() {
                continue;
            }
            let (tok, idx) = linea.rsplit_once(' ').with_context(|| {
                format!("vocab.txt mal formado (sin espacio separador): {linea}")
            })?;
            let idx: usize = idx.parse().context("índice de vocabulario no numérico")?;
            if idx >= tokens.len() {
                tokens.resize(idx + 1, String::new());
            }
            tokens[idx] = tok.replace('▁', " ");
        }
        let blank = tokens
            .iter()
            .position(|t| t == "<blk>")
            .context("vocab.txt no contiene el token <blk> (blank)")?;

        Ok(Self {
            pre: std::sync::Mutex::new(pre),
            enc: std::sync::Mutex::new(enc),
            dj: std::sync::Mutex::new(dj),
            pre_in,
            enc_in,
            dj_in,
            vocab_size: tokens.len(),
            blank,
            tokens,
        })
    }
}

impl SttEngine for ParakeetEngine {
    fn transcribe(&self, audio_pcm: &[i16], _language: Option<&str>) -> anyhow::Result<String> {
        // i16 mono 16 kHz → f32 normalizado a [-1, 1] (el preprocesador NeMo
        // espera float con amplitud normalizada a i16::MAX).
        let samples: Vec<f32> = audio_pcm
            .iter()
            .map(|s| *s as f32 / i16::MAX as f32)
            .collect();

        // 1) Features: waveform [1,S] + lens [1] -> features [1,128,T], lens [1].
        // El preexport expone los outputs en orden: [0]=features [1,128,T],
        // [1]=lengths; se asume por contrato del modelo (ver spike).
        let (t_frames, feats, feat_len) = {
            let t_wave =
                ort::value::Tensor::from_array(([1i64, samples.len() as i64], samples.clone()))?;
            let t_lens = ort::value::Tensor::from_array(([1i64], vec![samples.len() as i64]))?;
            let mut pre = self.pre.lock().unwrap();
            let pre_out = pre.run(inputs![
                self.pre_in[0].as_str() => t_wave,
                self.pre_in[1].as_str() => t_lens,
            ])?;
            let (shape_feat, feats_data) = flat_f32(&pre_out[0]);
            let lens_feat = flat_i64(&pre_out[1]);
            let _ = shape_feat;
            (lens_feat[0] as usize, feats_data, lens_feat[0])
            // `feat_len` (3º elem) es `i64` por contrato de `flat_i64`;
            // usarse directamente en `Tensor::from_array([1i64], ...)` sin cast.
        };

        // 2) Encoder: audio_signal [1,128,T] + length -> outputs [1,DIM,T'],
        //    encoded_lengths. El export int8 consume [B, DIM, T'] SIN
        //    transponer (a diferencia del fp32 de onnx-asr). Outputs en orden:
        //    [0]=encoder_outputs, [1]=encoded_lengths.
        let (enc_len, dim_enc, total_steps, enc_flat) = {
            let t_feat = ort::value::Tensor::from_array(([1i64, 128, t_frames as i64], feats))?;
            let t_flens = ort::value::Tensor::from_array(([1i64], vec![feat_len]))?;
            let mut enc = self.enc.lock().unwrap();
            let enc_out = enc.run(inputs![
                self.enc_in[0].as_str() => t_feat,
                self.enc_in[1].as_str() => t_flens,
            ])?;
            let (enc_shape, enc_data) = flat_f32(&enc_out[0]);
            let enc_len_val = flat_i64(&enc_out[1])[0].min(enc_shape[2]) as usize;
            (
                enc_len_val,
                enc_shape[1] as usize,
                enc_shape[2] as usize,
                enc_data,
            )
        };

        // 3) TDT greedy sobre decoder_joint.
        //    Nota: el spike indexa outputs por nombre ("outputs",
        //    "output_states_1", "output_states_2"); aquí usamos posición para
        //    no depender del orden exacto de `outputs()` almacenado en `dj_in`.
        let states_shape = vec![PRED_LAYERS, 1, PRED_HIDDEN];
        let mut s1 = vec![0f32; (PRED_LAYERS * PRED_HIDDEN) as usize];
        let mut s2 = s1.clone();
        let mut hypothesis: Vec<usize> = Vec::new();
        let mut t = 0usize;
        let mut emitted = 0usize;
        while t < enc_len {
            // Columna t del buffer plano [DIM, T'] (layout [B, DIM, T']).
            let frame: Vec<f32> = (0..dim_enc)
                .map(|d| enc_flat[d * total_steps + t])
                .collect();
            let token_in = *hypothesis.last().unwrap_or(&self.blank);
            let (logits_out, state1, state2) =
                self.run_dj(&frame, dim_enc, token_in, &s1, &s2, &states_shape)?;
            let (token_logits_out, duration_logits) = logits_out.split_at(self.vocab_size);
            let token = argmax(token_logits_out);
            let step = argmax(duration_logits);
            if token != self.blank {
                hypothesis.push(token);
                s1.copy_from_slice(&state1);
                s2.copy_from_slice(&state2);
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank || emitted >= MAX_TOKENS_PER_STEP {
                t += 1;
                emitted = 0;
            }
        }

        Ok(hypothesis
            .iter()
            .map(|&i| self.tokens[i].clone())
            .collect::<Vec<_>>()
            .join("")
            .trim()
            .to_string())
    }
}

impl ParakeetEngine {
    fn run_dj(
        &self,
        frame: &[f32],
        dim: usize,
        tok_in: usize,
        s1: &[f32],
        s2: &[f32],
        states_shape: &[i64],
    ) -> anyhow::Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let t_e = ort::value::Tensor::from_array(([1i64, dim as i64, 1], frame.to_vec()))?;
        // targets/target_length son int32 en este export (NO i64).
        let t_tok = ort::value::Tensor::from_array(([1i64, 1], vec![tok_in as i32]))?;
        let t_tlen = ort::value::Tensor::from_array(([1i64], vec![1i32]))?;
        let t_s1 = ort::value::Tensor::from_array((states_shape.to_vec(), s1.to_vec()))?;
        let t_s2 = ort::value::Tensor::from_array((states_shape.to_vec(), s2.to_vec()))?;

        let (logits_out, state1, state2) = {
            let mut dj = self.dj.lock().unwrap();
            let outs = dj.run(inputs![
                self.dj_in[0].as_str() => t_e,
                self.dj_in[1].as_str() => t_tok,
                self.dj_in[2].as_str() => t_tlen,
                self.dj_in[3].as_str() => t_s1,
                self.dj_in[4].as_str() => t_s2,
            ])?;
            // El decoder_joint expone "outputs" (logits), "output_states_1"/
            // "output_states_2" (prednet LSTM). Se resuelven por nombre (robusto
            // al orden devuelto por `outputs()`). El guard del `dj` mantiene el
            // lifetime de `outs` vigente dentro de este bloque.
            let logits_out = flat_f32(&outs["outputs"]).1;
            let state1 = flat_f32(&outs["output_states_1"]).1;
            (logits_out, state1, flat_f32(&outs["output_states_2"]).1)
        };
        Ok((logits_out, state1, state2))
    }
}

pub(crate) fn names(outputs: &[ort::value::Outlet]) -> Vec<String> {
    outputs.iter().map(|o| o.name().to_string()).collect()
}

fn flat_f32(v: &ort::value::DynValue) -> (Vec<i64>, Vec<f32>) {
    let (shape, data) = v.try_extract_tensor::<f32>().expect("tensor f32");
    (shape.to_vec(), data.to_vec())
}

fn flat_i64(v: &ort::value::DynValue) -> Vec<i64> {
    let (_, data) = v.try_extract_tensor::<i64>().expect("tensor i64");
    data.to_vec()
}

fn argmax(v: &[f32]) -> usize {
    v.iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn session(path: impl AsRef<Path>) -> anyhow::Result<Session> {
    // rc.13: `Session::builder()` y cada `.with_*` devuelven `Result<SessionBuilder>`.
    let b =
        Session::builder().map_err(|e| anyhow::anyhow!("construcción de la sesión ONNX: {e}"))?;
    let b = b
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| anyhow::anyhow!("nivel de optimización: {e}"))?;
    let mut b = b
        .with_intra_threads(available_threads() as usize)
        .map_err(|e| anyhow::anyhow!("hilos intra-op: {e}"))?;
    let s = b
        .commit_from_file(path.as_ref())
        .map_err(|e| anyhow::anyhow!("carga del modelo: {e}"))?;
    Ok(s)
}

/// Detector heurístico de idioma: ratio de palabras funcionales inglesas sobre
/// el total. Si supera el umbral, la transcripción probablemente salió en inglés
/// aunque la sesión sea en español (riesgo conocido de la auto-detección del
/// decoder Parakeet). Se expone públicamente para reutilizarlo en el daemon.
pub fn detect_language(text: &str) -> (&'static str, f64) {
    const ENGLISH: &[&str] = &[
        "the", "and", "you", "how", "are", "is", "what", "of", "to", "in", "that", "it", "with",
        "for", "on", "this", "be", "have", "from", "not", "my", "your", "we", "can", "will", "do",
        "was", "hello", "thanks", "please", "i'm", "hey",
    ];
    const SPANISH: &[&str] = &[
        "el", "la", "los", "las", "de", "que", "y", "en", "un", "una", "es", "por", "con", "no",
        "se", "del", "su", "al", "lo", "como", "más", "pero", "sus", "me", "ya", "o", "si", "muy",
        "sin", "sobre", "este", "también", "hola", "gracias", "mi", "son", "año", "años",
    ];
    let words: Vec<String> = normalize_text(text)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if words.is_empty() {
        return ("vacio", 0.0);
    }
    let hits_in = words
        .iter()
        .filter(|p| ENGLISH.contains(&p.as_str()))
        .count();
    let hits_es = words
        .iter()
        .filter(|p| SPANISH.contains(&p.as_str()))
        .count();
    let ratio = hits_in as f64 / words.len() as f64;
    if ratio >= 0.30 && hits_in > hits_es {
        ("EN-SOSPECHOSO", ratio)
    } else {
        ("es", ratio)
    }
}

/// Normaliza texto: minúsculas, plegado de diacríticos (á→a, …) y `ñ`→`n`,
/// eliminando puntuación. Compartida con el módulo de tests.
pub fn normalize_text(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'ä' => 'a',
            'é' | 'ë' => 'e',
            'í' | 'ï' => 'i',
            'ó' | 'ö' => 'o',
            'ú' | 'ü' => 'u',
            'ñ' => 'n',
            c if c.is_ascii_alphanumeric() => c,
            _ => ' ',
        })
        .collect()
}
