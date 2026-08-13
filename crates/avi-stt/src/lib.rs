//! Motor STT (speech-to-text) real sobre `ct2rs::Whisper`.
//!
//! Expone `Ct2SttEngine`, implementación de `avi_core::engine::SttEngine` que
//! carga un modelo Whisper convertido a CT2 y transcribe PCM `i16` mono a
//! 16 kHz forzando el idioma indicado (Whisper solo transcribe, nunca traduce,
//! por construcción del tipo `ct2rs::Whisper`).

use avi_core::engine::SttEngine;
use ct2rs::{Config, Whisper};

/// Motor STT real sobre un modelo Whisper cargado en formato CT2.
pub struct Ct2SttEngine {
    whisper: Whisper,
}

impl Ct2SttEngine {
    /// Carga el modelo Whisper CT2 ubicado en `model_dir`.
    pub fn new(model_dir: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let whisper = Whisper::new(model_dir, Config::default())?;
        Ok(Self { whisper })
    }
}

impl SttEngine for Ct2SttEngine {
    fn transcribe(&self, audio_pcm: &[i16], language: Option<&str>) -> anyhow::Result<String> {
        let samples: Vec<f32> = audio_pcm
            .iter()
            .map(|&s| s as f32 / i16::MAX as f32)
            .collect();
        let transcripts = self.whisper.generate(&samples, language, false, &Default::default())?;
        Ok(transcripts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use ct2rs::{Config, Translator};

    /// Smoke test: fuerza la resolución del enlace con `ct2rs`/CTranslate2
    /// (compilado desde fuente vía CMake+MSVC) sin requerir un modelo en disco.
    /// `Config` es el tipo de configuración que exige `Translator::new`; construirlo
    /// con sus valores por defecto ya obliga al linker a resolver los símbolos de
    /// CTranslate2, demostrando que el toolchain nativo (Tarea 2) compila la
    /// dependencia correctamente.
    #[test]
    fn ct2rs_enlaza_correctamente() {
        let _cfg = Config::default();
    }

    /// Etapa 2 (Tarea 7 delegada): carga un modelo CT2 real (opus-mt es→en, ya
    /// convertido y validado en disco) vía `Translator::new` y ejecuta una
    /// traducción corta, verificando que la salida no esté vacía.
    ///
    /// NOTA TÉCNICA para la Fase 4 (implementación real del motor de traducción):
    /// el encoder Marian/opus-mt exige el token `</s>` al final de la secuencia de
    /// origen. `ct2-transformers-converter` NO lo añade automáticamente
    /// (`config.json` del modelo trae `"add_source_eos": false`); sin ese token el
    /// decoder nunca converge a una traducción coherente. El motor real de Fase 4
    /// deberá anexar `</s>` explícitamente al texto/tokens de origen antes de
    /// invocar `translate_batch` (o el tokenizador SentencePiece equivalente debe
    /// insertarlo).
    #[test]
    fn ct2rs_carga_modelo_opus_mt_y_traduce() {
        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/ct2/opus-mt-es-en"
        );

        let translator = Translator::new(model_dir, &Config::default())
            .expect("el modelo opus-mt-es-en debe cargar pesos CT2 reales desde disco");

        // Se anexa `</s>` manualmente al origen: ver nota técnica arriba. El
        // tokenizador SentencePiece embebido (feature `sentencepiece`, activada
        // por defecto en ct2rs) reconoce `</s>` como símbolo de vocabulario
        // (confirmado en `shared_vocabulary.json`), no como texto literal.
        let sources = vec!["Hola, ¿cómo estás? </s>"];
        let results = translator
            .translate_batch(&sources, &Default::default(), None)
            .expect("translate_batch debe ejecutar sobre el modelo cargado");

        assert!(!results.is_empty(), "debe producirse al menos un resultado");
        let (translated, _) = &results[0];
        assert!(
            !translated.trim().is_empty(),
            "la traducción no debe estar vacía"
        );
    }

    /// Etapa 3 (Tarea 7 delegada, cierre): carga el modelo Whisper convertido a CT2
    /// (`models/ct2/whisper-small`) vía `ct2rs::Whisper` y transcribe una muestra corta
    /// de voz real, verificando que la salida no esté vacía. Complementa al test de
    /// opus-mt: demuestra que el toolchain `ct2rs` carga tanto un modelo de traducción
    /// (Marian) como uno de transcripción (Whisper), la cobertura mínima que el motor
    /// STT real de la Fase 3 asumirá provisionada.
    ///
    /// El fixture `tests/assets/whisper_sample_16k.wav` es voz sintética en español
    /// generada por el propio motor Qwen3-TTS del proyecto (smoke de la Tarea 10),
    /// remuestreada a 16 kHz mono — la tasa que exige Whisper.
    #[test]
    fn ct2rs_carga_modelo_whisper_y_transcribe() {
        use ct2rs::Whisper;

        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/ct2/whisper-small"
        );

        let whisper = Whisper::new(model_dir, Config::default())
            .expect("el modelo whisper-small debe cargar pesos CT2 reales desde disco");

        // El fixture ya está a la tasa de muestreo que espera el modelo (16 kHz).
        let wav_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/whisper_sample_16k.wav"
        );
        let mut reader = hound::WavReader::open(wav_path)
            .expect("el WAV fixture debe abrirse");
        assert_eq!(
            reader.spec().sample_rate as usize,
            whisper.sampling_rate(),
            "el fixture debe estar a la tasa de muestreo que exige Whisper"
        );
        // PCM 16-bit → f32 normalizado a [-1, 1].
        let samples: Vec<f32> = reader
            .samples::<i16>()
            .map(|s| s.expect("muestra PCM válida") as f32 / i16::MAX as f32)
            .collect();

        let transcripts = whisper
            .generate(&samples, Some("es"), false, &Default::default())
            .expect("generate debe transcribir sobre el modelo cargado");

        let texto: String = transcripts.join(" ");
        assert!(
            !texto.trim().is_empty(),
            "la transcripción no debe estar vacía"
        );
    }

    /// `Ct2SttEngine::new` sobre una ruta de modelo inexistente debe devolver
    /// `Err`, cubriendo la rama de "modelo no cargable" que el handler CLI
    /// (Tarea 6) mapea a `ExitCode::TranscriptionFailed` (10).
    #[test]
    fn ct2sttengine_new_con_ruta_inexistente_devuelve_err() {
        use crate::Ct2SttEngine;

        let result = Ct2SttEngine::new("ruta/que/no/existe/whisper-small");
        assert!(result.is_err(), "una ruta de modelo inexistente debe fallar");
    }

    /// Test de paridad contra el oráculo Python (Tarea 4 del plan F3).
    ///
    /// IGNORADO: la precondición de la Tarea 4 (oráculo Python ejecutable de
    /// extremo a extremo) NO se cumplió en F4 — el `WhisperModelLoader` del
    /// oráculo resuelve el modelo en `<data_root>/transcription-models/
    /// faster-whisper-small` (formato faster-whisper, directorio de datos de
    /// usuario), no en `models/ct2/whisper-small` (formato CT2, raíz del
    /// repo), y ese directorio no está provisionado en este entorno. Detalle
    /// completo del bloqueo en `.claude/orchestration/fase3-stt-whisper/
    /// F4-implementacion-notas.md`. La paridad se difiere al reality-check de
    /// F5.
    #[test]
    #[ignore = "fixture de paridad no generado: ver F4-implementacion-notas.md"]
    fn ct2sttengine_coincide_con_oraculo_python() {
        use crate::Ct2SttEngine;
        use avi_core::engine::SttEngine;

        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/ct2/whisper-small"
        );
        let wav_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/whisper_sample_16k.wav"
        );
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/whisper_sample_16k.oraculo.txt"
        );

        let engine = Ct2SttEngine::new(model_dir).expect("el modelo whisper-small debe cargar");
        let pcm = avi_audio::load_wav_16k_mono_pcm(wav_path).expect("el WAV fixture debe cargarse");
        let actual = engine
            .transcribe(&pcm, Some("es"))
            .expect("la transcripción debe completarse")
            .trim()
            .to_string();

        let esperado = std::fs::read_to_string(fixture_path)
            .expect("el fixture de referencia del oráculo debe existir")
            .trim()
            .to_string();

        if actual == esperado {
            return;
        }

        // Igualdad textual no se cumple: umbral de paridad por WER a nivel de
        // palabra (distancia de Levenshtein entre secuencias de palabras).
        let ref_palabras: Vec<&str> = esperado.split_whitespace().collect();
        let hip_palabras: Vec<&str> = actual.split_whitespace().collect();
        let distancia = levenshtein_palabras(&ref_palabras, &hip_palabras);
        let wer = distancia as f64 / ref_palabras.len().max(1) as f64;

        assert!(
            wer <= 0.05,
            "WER {:.4} supera el umbral de paridad 0.05 (esperado: {:?}, obtenido: {:?})",
            wer,
            esperado,
            actual
        );
    }

    /// Distancia de Levenshtein a nivel de palabra entre `referencia` e
    /// `hipotesis`, usada para calcular el WER de la prueba de paridad.
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
}
