// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Motor STT (speech-to-text) real sobre Parakeet TDT 0.6B v3 int8 vía
//! `ort`/ONNX Runtime.
//!
//! Exige la feature `native-stt` (y su dependencia `ort`) para compilar; sin
//! ella el crate es una capa vacía y `cargo test`/`cargo llvm-cov` no pagan el
//! costo de compilar ONNX Runtime.

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

#[cfg(feature = "native-stt")]
pub mod parakeet;
#[cfg(feature = "native-stt")]
pub use parakeet::{normalize_text, EngineHolder, ParakeetEngine};

#[cfg(all(test, feature = "native-stt"))]
mod tests {
    use crate::{normalize_text, EngineHolder, ParakeetEngine};
    use avi_core::engine::SttEngine;

    /// Un lock envenenado por un pánico con el guard vivo se recupera: la
    /// siguiente toma devuelve el guard con el valor intacto en vez de fallar
    /// para siempre.
    #[test]
    fn poisoned_session_lock_is_recovered() {
        let lock = std::sync::Arc::new(std::sync::Mutex::new(7));
        let poisoner = std::sync::Arc::clone(&lock);
        let panicked = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("pánico con el guard vivo");
        })
        .join();
        assert!(panicked.is_err());
        assert!(lock.is_poisoned());
        assert_eq!(*crate::parakeet::lock_session(&lock), 7);
    }

    /// Directorio del snapshot de Parakeet, exigiendo `nemo128.onnx`: la raíz de
    /// modelos vive fuera del repo y la provisiona `ai-voice-interconnector setup`.
    fn require_parakeet() -> std::path::PathBuf {
        let model_dir = avi_store::ModelStore::new()
            .expect("la raíz de modelos debe resolverse")
            .model_snapshot_path("parakeet-tdt-v3")
            .expect("el modelo parakeet-tdt-v3 debe tener un pin de revisión");
        let preprocessor = model_dir.join("nemo128.onnx");
        std::fs::metadata(&preprocessor).unwrap_or_else(|e| {
            panic!(
                "falta {} ({e}): provisiona Parakeet con `ai-voice-interconnector setup`",
                preprocessor.display()
            )
        });
        model_dir
    }

    /// Carga el modelo Parakeet (raíz de modelos `avi_shared::paths::models_cache_dir()` vía
    /// `ModelStore`) vía `ParakeetEngine` y transcribe una muestra corta de voz
    /// real, verificando que la salida no esté vacía.
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere Parakeet"]
    fn parakeet_loads_model_and_transcribes() {
        let model_dir = require_parakeet();
        let engine = ParakeetEngine::new(model_dir)
            .expect("el modelo Parakeet debe cargar pesos reales desde disco");

        // El fixture ya está a la tasa de muestreo que exige el modelo (16 kHz).
        let wav_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/parakeet_sample_16k.wav"
        );
        let pcm = avi_audio::load_wav_16k_mono_pcm(std::path::Path::new(wav_path))
            .expect("el WAV fixture debe cargarse");

        let text = engine
            .transcribe(&pcm)
            .expect("transcribe debe ejecutar sobre el modelo cargado");
        assert!(
            !text.trim().is_empty(),
            "la transcripción no debe estar vacía"
        );
    }

    /// `ParakeetEngine::new` sobre una ruta de modelo inexistente debe
    /// devolver `Err`, cubriendo la rama de "modelo no cargable" que el
    /// handler CLI mapea a `ExitCode::TranscriptionFailed` (10).
    #[cfg(feature = "native-stt")]
    #[test]
    fn parakeet_engine_new_with_nonexistent_path_returns_err() {
        let result = ParakeetEngine::new("path/that/does/not/exist/parakeet");
        assert!(
            result.is_err(),
            "una ruta de modelo inexistente debe fallar"
        );
    }

    /// El titular devuelve siempre la misma instancia y exige los mismos
    /// límites de concurrencia que el motor (`Send` + `Sync`), de modo que
    /// varios hilos pueden compartirlo sin destruirlo nunca.
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere Parakeet"]
    fn process_holder_returns_shared_instance() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<EngineHolder>();
        assert_send_sync::<ParakeetEngine>();
        let holder = EngineHolder::new();
        let dir = require_parakeet();
        let first = holder.get_or_try_init(&dir).expect("el motor debe cargar");
        let second = holder
            .get_or_try_init(&dir)
            .expect("la segunda resolución reutiliza");
        assert!(
            std::ptr::eq(first, second),
            "el titular debe devolver siempre la misma instancia"
        );
    }

    /// El fallo de carga queda retenido: repetir la resolución sobre el mismo
    /// titular devuelve el error sin reintentar. No necesita modelos y corre
    /// siempre, como la prueba del constructor con ruta inexistente.
    #[cfg(feature = "native-stt")]
    #[test]
    fn process_holder_caches_load_failure() {
        let holder = EngineHolder::new();
        assert!(
            holder
                .get_or_try_init("path/that/does/not/exist/parakeet")
                .is_err(),
            "una ruta inexistente debe fallar sin crear instancia"
        );
        assert!(
            holder
                .get_or_try_init("path/that/does/not/exist/parakeet")
                .is_err(),
            "la segunda resolución repite el fallo retenido"
        );
    }

    /// Test de paridad contra texto verificado.
    ///
    /// El corpus de referencia son audios de voz sintética en español
    /// generada por el motor Qwen3-TTS del proyecto (remuestreados a 16 kHz
    /// mono, la tasa que exige Parakeet), con su transcripción de referencia
    /// verificada manualmente como ground truth. La comparación normaliza ambos
    /// lados a minúsculas sin diacríticos ni puntuación (insensible a acentos y
    /// signos), y acepta una tasa de error de palabras ≤ 0.25 sobre el texto normalizado.
    ///
    /// `parakeet_sample_16k` es un saludo corto
    /// (2.96 s) que Parakeet emite en **inglés** ("Hello, how are you?"): el
    /// TDT 0.6B v3 auto-detecta idioma y un saludo breve es fonéticamente
    /// ambiguo. Por eso los corpus en español (`corpus_watermark`,
    /// `corpus_synthesis`, `corpus_responses`) validan una tasa de error de palabras
    /// ≤ 0.25 (el modelo
    /// real alcanza 0.08–0.21) y `parakeet_sample` solo exige una
    /// transcripción no vacía.
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere Parakeet"]
    fn parakeet_engine_matches_reference() {
        let model_dir = require_parakeet();
        let engine = ParakeetEngine::new(model_dir).expect("el modelo Parakeet debe cargar");

        // El directorio `tests/assets/` de este crate. `parakeet_sample_16k`
        // es un saludo breve que el modelo emite en inglés y solo exige una
        // transcripción no vacía; los corpus restantes son español estable y
        // validan paridad por la tasa de error de palabras.
        let sample = engine
            .transcribe(
                &avi_audio::load_wav_16k_mono_pcm(
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("tests/assets")
                        .join("parakeet_sample_16k.wav"),
                )
                .unwrap_or_else(|_| panic!("el WAV fixture parakeet_sample_16k.wav debe cargarse")),
            )
            .expect("la transcripción debe completarse");
        assert!(
            !sample.trim().is_empty(),
            "parakeet_sample debe transcribirse sin vaciarse"
        );

        let corpus: [(&str, &str); 3] = [
            (
                "corpus_watermark_16k.wav",
                "corpus_watermark_16k.reference.txt",
            ),
            (
                "corpus_synthesis_16k.wav",
                "corpus_synthesis_16k.reference.txt",
            ),
            (
                "corpus_responses_16k.wav",
                "corpus_responses_16k.reference.txt",
            ),
        ];

        for (wav, fixture) in corpus {
            let wav_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(wav);
            let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(fixture);

            let pcm = avi_audio::load_wav_16k_mono_pcm(&wav_path)
                .unwrap_or_else(|_| panic!("el WAV fixture {wav} debe cargarse"));
            let actual = engine
                .transcribe(&pcm)
                .expect("la transcripción debe completarse")
                .trim()
                .to_string();

            let expected = std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|_| panic!("el fixture de referencia {fixture} debe existir"))
                .trim()
                .to_string();

            // Ambos lados normalizados: minúsculas, sin diacríticos ni
            // puntuación (la tasa de error de palabras no debe penalizar acentos
            // ni signos).
            let expected_normalized = normalize_text(&expected);
            let actual_normalized = normalize_text(&actual);
            let ref_words: Vec<&str> = expected_normalized.split_whitespace().collect();
            let hyp_words: Vec<&str> = actual_normalized.split_whitespace().collect();

            if ref_words == hyp_words {
                continue;
            }

            // Igualdad normalizada no se cumple: umbral de paridad por la tasa de
            // error de palabras (distancia de Levenshtein entre secuencias).
            let distance = levenshtein_words(&ref_words, &hyp_words);
            let word_error_rate = distance as f64 / ref_words.len().max(1) as f64;

            // el modelo Parakeet-TDT 0.6B int8 (export istupakov) alcanza
            // RTF ~0.10 pero una tasa de error de palabras real de ~0.08–0.21 sobre
            // fixtures de voz sintética. Se valida paridad con ≤ 0.25, que abarca
            // el valor observado (0.083 watermark incluido el "jejeje" inicial no
            // reflejado en el fixture, 0.214 sintesis, 0.111 respuestas).
            let threshold = 0.25;
            assert!(
                word_error_rate <= threshold,
                "la tasa de error de palabras {:.4} supera el umbral de paridad {} en {} (esperado: {:?}, obtenido: {:?})",
                word_error_rate,
                threshold,
                wav,
                expected_normalized,
                actual_normalized
            );
        }
    }

    /// Distancia de Levenshtein a nivel de palabra entre `reference` e
    /// `hypothesis`, usada para calcular la tasa de error de palabras de la prueba
    /// de paridad.
    #[cfg(feature = "native-stt")]
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
}
