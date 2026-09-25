// Todo el benchmark ejercita el motor real (`avi_stt::ParakeetEngine`), que
// solo existe con el feature `native-stt`. Sin él, el archivo no se compila
// (evita ONNX Runtime en el build de test liso).
#![cfg(feature = "native-stt")]

use std::time::Instant;

use avi_core::engine::SttEngine;

/// Benchmark opt-in de latencia y calidad del motor STT (Parakeet TDT v3 int8
/// vía ort, greedy, hilos físicos) sobre los WAVs del repo.
///
/// Mide: tiempo de carga del modelo, latencia por transcripción (min/mediana/
/// max sobre 5 medidas tras 2 warmups), RTF vs duración del audio y WER
/// normalizado contra el texto correcto verificado.
///
/// Ejecutar con: `cargo test -p avi-stt --test benchmark_latencia -- --ignored --nocapture`
#[test]
#[ignore]
fn benchmark_latency_quality() {
    let model_dir = avi_store::ModelStore::new()
        .model_snapshot_path("parakeet-tdt-v3")
        .expect("snapshot HF parakeet-tdt-v3 no provisionado — ejecuta setup --with-stt");
    let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets");
    let files = [
        (
            "parakeet_sample_16k.wav",
            "¡Hola! ¿Cómo estás?",
            "parakeet_sample_16k.oraculo.txt",
        ),
        (
            "corpus_sintesis_16k.wav",
            "Sistema de síntesis de voz completamente local con clonación de voz en español latinoamericano.",
            "corpus_sintesis_16k.oraculo.txt",
        ),
        (
            "corpus_watermark_16k.wav",
            "Recuerda que el audio no contiene marca de agua que lo identifique.",
            "corpus_watermark_16k.oraculo.txt",
        ),
        (
            "corpus_respuestas_16k.wav",
            "Las respuestas dirigidas al usuario deben estar en espejo.",
            "corpus_respuestas_16k.oraculo.txt",
        ),
    ];

    let t0 = Instant::now();
    let engine = avi_stt::ParakeetEngine::new(model_dir).expect("cargar Parakeet int8");
    println!("CARGA_MODELO_MS={}", t0.elapsed().as_millis());

    for (name, expected, _fixture) in files {
        let path = format!("{assets}/{name}");
        let pcm = avi_audio::load_wav_16k_mono_pcm(&path).expect("wav valido");
        let duration_s = pcm.len() as f64 / 16000.0;

        for _ in 0..2 {
            let _ = engine.transcribe(&pcm, Some("es")).expect("warmup");
        }

        let mut samples_ms = Vec::new();
        let mut text = String::new();
        for _ in 0..5 {
            let t = Instant::now();
            text = engine.transcribe(&pcm, Some("es")).expect("transcripcion");
            samples_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }

        samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (min, med, max) = (samples_ms[0], samples_ms[2], samples_ms[4]);
        let rtf = med / 1000.0 / duration_s;
        let w = wer_text(expected, &text);
        let has_tildes = text.contains(['á', 'é', 'í', 'ó', 'ú', 'ü', 'ñ']);
        let punctuation = text.chars().any(|c| "¿¡,.;:!?()\"'".contains(c));

        println!(
            "{name}|DUR={duration_s:.2}s|MIN={min:.1}ms|MED={med:.1}ms|MAX={max:.1}ms|RTF={rtf:.2}|WER={w:.4}|TILDES={has_tildes}|PUNTUACION={punctuation}"
        );
        println!("  TEXTO={text:?}");
    }
}

fn normalize_text(text: &str) -> String {
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

fn levenshtein_words(reference: &[&str], hypothesis: &[&str]) -> usize {
    let n = reference.len();
    let m = hypothesis.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in dp.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in dp[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            if reference[i - 1] == hypothesis[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            } else {
                dp[i][j] = 1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1]);
            }
        }
    }
    dp[n][m]
}

fn wer_text(reference: &str, hypothesis: &str) -> f64 {
    let ref_normalized = normalize_text(reference);
    let hyp_normalized = normalize_text(hypothesis);
    let r: Vec<&str> = ref_normalized.split_whitespace().collect();
    let h: Vec<&str> = hyp_normalized.split_whitespace().collect();
    if r == h {
        return 0.0;
    }
    let distance = levenshtein_words(&r, &h);
    distance as f64 / r.len().max(1) as f64
}
