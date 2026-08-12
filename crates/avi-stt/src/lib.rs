//! Crate stub para el motor STT (speech-to-text) basado en `ct2rs`/Whisper.
//!
//! La implementación real llega en la Fase 3 del plan de migración
//! (`docs/proposals/PLAN-DE-MIGRACIÓN.md`). Este crate solo reserva el nombre
//! y la posición en el workspace.

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
}
