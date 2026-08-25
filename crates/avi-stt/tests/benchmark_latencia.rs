// Todo el benchmark ejercita el motor real (`avi_stt::Ct2SttEngine`), que solo
// existe con el feature `native-stt`. Sin él, el archivo no se compila (evita el
// C++ de whisper.cpp en el build de test liso).
#![cfg(feature = "native-stt")]

use std::time::Instant;

use avi_core::engine::SttEngine;

/// Benchmark opt-in de latencia y calidad del motor STT (whisper-rs, GGUF
/// medium-q8, greedy, 8 hilos) sobre los WAVs del repo.
///
/// Mide: tiempo de carga del modelo, latencia por transcripción (min/mediana/
/// max sobre 5 medidas tras 2 warmups), RTF vs duración del audio y WER
/// normalizado contra el texto correcto verificado.
///
/// Ejecutar con: `cargo test -p avi-stt --test benchmark_latencia -- --ignored --nocapture`
#[test]
#[ignore]
fn benchmark_latencia_calidad() {
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../models/whisper/ggml-medium-q8_0.bin"
    );
    let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/assets");
    let archivos = [
        (
            "whisper_sample_16k.wav",
            "¡Hola! ¿Cómo estás?",
            "whisper_sample_16k.oraculo.txt",
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
    let engine = avi_stt::Ct2SttEngine::new(model_path).expect("cargar GGUF medium-q8");
    println!("CARGA_MODELO_MS={}", t0.elapsed().as_millis());

    for (nombre, correcto, _fixture) in archivos {
        let ruta = format!("{assets}/{nombre}");
        let pcm = avi_audio::load_wav_16k_mono_pcm(&ruta).expect("wav valido");
        let duracion_s = pcm.len() as f64 / 16000.0;

        for _ in 0..2 {
            let _ = engine.transcribe(&pcm, Some("es")).expect("warmup");
        }

        let mut muestras_ms = Vec::new();
        let mut texto = String::new();
        for _ in 0..5 {
            let t = Instant::now();
            texto = engine.transcribe(&pcm, Some("es")).expect("transcripcion");
            muestras_ms.push(t.elapsed().as_secs_f64() * 1000.0);
        }

        muestras_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let (min, med, max) = (muestras_ms[0], muestras_ms[2], muestras_ms[4]);
        let rtf = med / 1000.0 / duracion_s;
        let w = wer_texto(correcto, &texto);
        let tildes = texto.contains(['á', 'é', 'í', 'ó', 'ú', 'ü', 'ñ']);
        let puntuacion = texto.chars().any(|c| "¿¡,.;:!?()\"'".contains(c));

        println!(
            "{nombre}|DUR={duracion_s:.2}s|MIN={min:.1}ms|MED={med:.1}ms|MAX={max:.1}ms|RTF={rtf:.2}|WER={w:.4}|TILDES={tildes}|PUNTUACION={puntuacion}"
        );
        println!("  TEXTO={texto:?}");
    }
}

fn normalizar_texto(texto: &str) -> String {
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

fn levenshtein_palabras(referencia: &[&str], hipotesis: &[&str]) -> usize {
    let n = referencia.len();
    let m = hipotesis.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in 0..=n {
        dp[i][0] = i;
    }
    for j in 0..=m {
        dp[0][j] = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            if referencia[i - 1] == hipotesis[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            } else {
                dp[i][j] = 1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1]);
            }
        }
    }
    dp[n][m]
}

fn wer_texto(referencia: &str, hipotesis: &str) -> f64 {
    let ref_norm = normalizar_texto(referencia);
    let hip_norm = normalizar_texto(hipotesis);
    let r: Vec<&str> = ref_norm.split_whitespace().collect();
    let h: Vec<&str> = hip_norm.split_whitespace().collect();
    if r == h {
        return 0.0;
    }
    let distancia = levenshtein_palabras(&r, &h);
    distancia as f64 / r.len().max(1) as f64
}
