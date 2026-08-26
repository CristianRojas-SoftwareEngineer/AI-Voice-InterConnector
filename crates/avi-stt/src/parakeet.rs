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

use avi_core::engine::{hilos_disponibles, SttEngine};

/// Dimensiones de la red de predicción del FastConformer-TDT 0.6B (capas LSTM × oculto).
const PRED_LAYERS: i64 = 2;
const PRED_HIDDEN: i64 = 640;
/// Tope de tokens emitidos por frame antes de avanzar (max_tokens_per_step de NeMo).
const MAX_TOKENS_POR_PASO: usize = 10;

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

        let pre_in = nombres(pre.inputs());
        let enc_in = nombres(enc.inputs());
        let dj_in = nombres(dj.inputs());

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
        let muestras: Vec<f32> = audio_pcm
            .iter()
            .map(|s| *s as f32 / i16::MAX as f32)
            .collect();

        // 1) Features: waveform [1,S] + lens [1] -> features [1,128,T], lens [1].
        // El preexport expone los outputs en orden: [0]=features [1,128,T],
        // [1]=lengths; se asume por contrato del modelo (ver spike).
        let (t_frames, feats, largo_feat) = {
            let t_wave =
                ort::value::Tensor::from_array(([1i64, muestras.len() as i64], muestras.clone()))?;
            let t_lens = ort::value::Tensor::from_array(([1i64], vec![muestras.len() as i64]))?;
            let mut pre = self.pre.lock().unwrap();
            let sal_pre = pre.run(inputs![
                self.pre_in[0].as_str() => t_wave,
                self.pre_in[1].as_str() => t_lens,
            ])?;
            let (forma_feat, feats_vec) = plano_f32(&sal_pre[0]);
            let lens_feat = plano_i64(&sal_pre[1]);
            let _ = forma_feat;
            (lens_feat[0] as usize, feats_vec, lens_feat[0])
            // `largo_feat` (3º elem) es `i64` por contrato de `plano_i64`;
            // usarse directamente en `Tensor::from_array([1i64], ...)` sin cast.
        };

        // 2) Encoder: audio_signal [1,128,T] + length -> outputs [1,DIM,T'],
        //    encoded_lengths. El export int8 consume [B, DIM, T'] SIN
        //    transponer (a diferencia del fp32 de onnx-asr). Outputs en orden:
        //    [0]=encoder_outputs, [1]=encoded_lengths.
        let (enc_len, dim_enc, pasos_totales, enc_plano) = {
            let t_feat = ort::value::Tensor::from_array(([1i64, 128, t_frames as i64], feats))?;
            let t_flens = ort::value::Tensor::from_array(([1i64], vec![largo_feat]))?;
            let mut enc = self.enc.lock().unwrap();
            let sal_enc = enc.run(inputs![
                self.enc_in[0].as_str() => t_feat,
                self.enc_in[1].as_str() => t_flens,
            ])?;
            let (forma_enc, enc_vec) = plano_f32(&sal_enc[0]);
            let enc_len_val = plano_i64(&sal_enc[1])[0].min(forma_enc[2]) as usize;
            (
                enc_len_val,
                forma_enc[1] as usize,
                forma_enc[2] as usize,
                enc_vec,
            )
        };

        // 3) TDT greedy sobre decoder_joint.
        //    Nota: el spike indexa outputs por nombre ("outputs",
        //    "output_states_1", "output_states_2"); aquí usamos posición para
        //    no depender del orden exacto de `outputs()` almacenado en `dj_in`.
        let estados_shape = vec![PRED_LAYERS, 1, PRED_HIDDEN];
        let mut s1 = vec![0f32; (PRED_LAYERS * PRED_HIDDEN) as usize];
        let mut s2 = s1.clone();
        let mut hipótesis: Vec<usize> = Vec::new();
        let mut t = 0usize;
        let mut emitidos = 0usize;
        while t < enc_len {
            // Columna t del buffer plano [DIM, T'] (layout [B, DIM, T']).
            let frame: Vec<f32> = (0..dim_enc)
                .map(|d| enc_plano[d * pasos_totales + t])
                .collect();
            let tok_in = *hipótesis.last().unwrap_or(&self.blank);
            let (logits, ns1, ns2) =
                self.run_dj(&frame, dim_enc, tok_in, &s1, &s2, &estados_shape)?;
            let (token_logits, dur_logits) = logits.split_at(self.vocab_size);
            let token = argmax(token_logits);
            let paso = argmax(dur_logits);
            if token != self.blank {
                hipótesis.push(token);
                s1.copy_from_slice(&ns1);
                s2.copy_from_slice(&ns2);
                emitidos += 1;
            }
            if paso > 0 {
                t += paso;
                emitidos = 0;
            } else if token == self.blank || emitidos >= MAX_TOKENS_POR_PASO {
                t += 1;
                emitidos = 0;
            }
        }

        Ok(hipótesis
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
        estados_shape: &[i64],
    ) -> anyhow::Result<(Vec<f32>, Vec<f32>, Vec<f32>)> {
        let t_e = ort::value::Tensor::from_array(([1i64, dim as i64, 1], frame.to_vec()))?;
        // targets/target_length son int32 en este export (NO i64).
        let t_tok = ort::value::Tensor::from_array(([1i64, 1], vec![tok_in as i32]))?;
        let t_tlen = ort::value::Tensor::from_array(([1i64], vec![1i32]))?;
        let t_s1 = ort::value::Tensor::from_array((estados_shape.to_vec(), s1.to_vec()))?;
        let t_s2 = ort::value::Tensor::from_array((estados_shape.to_vec(), s2.to_vec()))?;

        let (logits, ns1, ns2) = {
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
            let logits = plano_f32(&outs["outputs"]).1;
            let ns1 = plano_f32(&outs["output_states_1"]).1;
            (logits, ns1, plano_f32(&outs["output_states_2"]).1)
        };
        Ok((logits, ns1, ns2))
    }
}

pub(crate) fn nombres(salidas: &[ort::value::Outlet]) -> Vec<String> {
    salidas.iter().map(|o| o.name().to_string()).collect()
}

fn plano_f32(v: &ort::value::DynValue) -> (Vec<i64>, Vec<f32>) {
    let (forma, datos) = v.try_extract_tensor::<f32>().expect("tensor f32");
    (forma.to_vec(), datos.to_vec())
}

fn plano_i64(v: &ort::value::DynValue) -> Vec<i64> {
    let (_, datos) = v.try_extract_tensor::<i64>().expect("tensor i64");
    datos.to_vec()
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
    let b = Session::builder().map_err(|e| anyhow::anyhow!("builder: {e}"))?;
    let b = b
        .with_optimization_level(GraphOptimizationLevel::Level1)
        .map_err(|e| anyhow::anyhow!("opt level: {e}"))?;
    let mut b = b
        .with_intra_threads(hilos_disponibles() as usize)
        .map_err(|e| anyhow::anyhow!("intra threads: {e}"))?;
    let s = b
        .commit_from_file(path.as_ref())
        .map_err(|e| anyhow::anyhow!("commit: {e}"))?;
    Ok(s)
}

/// Detector heurístico de idioma: ratio de palabras funcionales inglesas sobre
/// el total. Si supera el umbral, la transcripción probablemente salió en inglés
/// aunque la sesión sea en español (riesgo conocido de la auto-detección del
/// decoder Parakeet). Se expone públicamente para reutilizarlo en el daemon.
pub fn detectar_idioma(texto: &str) -> (&'static str, f64) {
    const INGLES: &[&str] = &[
        "the", "and", "you", "how", "are", "is", "what", "of", "to", "in", "that", "it", "with",
        "for", "on", "this", "be", "have", "from", "not", "my", "your", "we", "can", "will", "do",
        "was", "hello", "thanks", "please", "i'm", "hey",
    ];
    const ESPANOL: &[&str] = &[
        "el", "la", "los", "las", "de", "que", "y", "en", "un", "una", "es", "por", "con", "no",
        "se", "del", "su", "al", "lo", "como", "más", "pero", "sus", "me", "ya", "o", "si", "muy",
        "sin", "sobre", "este", "también", "hola", "gracias", "mi", "son", "año", "años",
    ];
    let palabras: Vec<String> = normalizar_texto(texto)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if palabras.is_empty() {
        return ("vacio", 0.0);
    }
    let hits_in = palabras
        .iter()
        .filter(|p| INGLES.contains(&p.as_str()))
        .count();
    let hits_es = palabras
        .iter()
        .filter(|p| ESPANOL.contains(&p.as_str()))
        .count();
    let ratio = hits_in as f64 / palabras.len() as f64;
    if ratio >= 0.30 && hits_in > hits_es {
        ("EN-SOSPECHOSO", ratio)
    } else {
        ("es", ratio)
    }
}

/// Normaliza texto: minúsculas, plegado de diacríticos (á→a, …) y `ñ`→`n`,
/// eliminando puntuación. Compartida con el módulo de tests.
pub fn normalizar_texto(texto: &str) -> String {
    texto
        .to_lowercase()
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
