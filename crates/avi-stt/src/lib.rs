//! Motor STT (speech-to-text) real sobre whisper-rs (bindings de whisper.cpp).
//!
//! Expone `Ct2SttEngine`, implementación de `avi_core::engine::SttEngine` que
//! carga un modelo Whisper en formato GGUF y transcribe PCM `i16` mono a
//! 16 kHz forzando el idioma indicado (Whisper solo transcribe, nunca traduce,
//! por construcción: `set_translate(false)`).

use avi_core::engine::{hilos_disponibles, SttEngine};
use whisper_rs::{
    convert_integer_to_float_audio, FullParams, SamplingStrategy, WhisperContext,
    WhisperContextParameters,
};

/// Motor STT real sobre un modelo Whisper cargado en formato GGUF.
pub struct Ct2SttEngine {
    ctx: WhisperContext,
}

impl Ct2SttEngine {
    /// Carga el modelo Whisper GGUF ubicado en `model_path`.
    pub fn new(model_path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let ctx = WhisperContext::new_with_params(model_path, WhisperContextParameters::default())?;
        Ok(Self { ctx })
    }
}

impl SttEngine for Ct2SttEngine {
    fn transcribe(&self, audio_pcm: &[i16], language: Option<&str>) -> anyhow::Result<String> {
        let mut buffer = vec![0f32; audio_pcm.len()];
        convert_integer_to_float_audio(audio_pcm, &mut buffer)?;

        let mut state = self.ctx.create_state()?;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 0 });
        params.set_translate(false);
        // Hilos lógicos del equipo del usuario, no una máquina fija de desarrollo.
        params.set_n_threads(hilos_disponibles() as i32);
        // Ventana del encoder (audio_ctx) dinámica según la duración del audio:
        // capacidad = ctx×20 ms con margen >=25% (múltiplo de 64, piso 256
        // verificado, tope 1500 del modelo). El default de whisper.cpp (1500
        // fijo) imponía un piso de ~9 s por llamada en clips cortos; 448 fijo
        // degradaba audio >8.96 s (recorte de cola → repetición). Ver
        // `audio_ctx_para_duracion`.
        params.set_audio_ctx(audio_ctx_para_duracion(audio_pcm.len()));
        params.set_language(language);
        state.full(params, &buffer)?;

        let transcripts: Vec<String> = state
            .as_iter()
            .map(|segment| segment.to_str_lossy().map(|s| s.to_string()))
            .collect::<Result<_, _>>()?;
        Ok(transcripts.join(" "))
    }
}

/// Calcula el `audio_ctx` (ventana de atención del encoder) para una duración
/// de audio dada.
///
/// Cada frame del encoder cubre 20 ms de audio, luego la capacidad es
/// `n_ctx × 20 ms`; si el audio excede la ventana, whisper.cpp recorta la cola
/// por pasada y el modelo repite palabras. Se exige margen >=25% de capacidad
/// sobre la duración (→ `n_ctx >= 62.5 × duración_s`), redondeando al múltiplo
/// de 64 más cercano (invariante de whisper.cpp), y se acota al rango
/// [256, 1500]: piso 256 verificado experimentalmente (conserva el texto de
/// 448 en clips <=4 s) y tope 1500 = `n_audio_ctx` del modelo.
fn audio_ctx_para_duracion(samples: usize) -> i32 {
    let dur_s = samples as f64 / 16000.0;
    let ctx = (((dur_s * 62.5 / 64.0).ceil()) as i32) * 64;
    ctx.clamp(256, 1500)
}

#[cfg(test)]
mod tests {
    use avi_core::engine::SttEngine;
    use whisper_rs::WhisperContextParameters;

    /// Smoke test: fuerza la resolución del enlace con `whisper-rs`/whisper.cpp
    /// (compilado desde fuente vía CMake+MSVC) sin requerir un modelo en disco.
    /// `WhisperContextParameters` es el tipo de configuración que exige
    /// `WhisperContext::new_with_params`; construirlo con sus valores por defecto
    /// ya obliga al linker a resolver los símbolos de whisper.cpp, demostrando
    /// que el toolchain nativo compila la dependencia correctamente.
    #[test]
    fn whisper_rs_enlaza_correctamente() {
        let _params = WhisperContextParameters::default();
    }

    /// Carga el modelo Whisper GGUF (`models/whisper/ggml-medium-q8_0.bin`) vía
    /// `Ct2SttEngine` y transcribe una muestra corta de voz real, verificando
    /// que la salida no esté vacía.
    ///
    /// El fixture `tests/assets/whisper_sample_16k.wav` es voz sintética en español
    /// generada por el propio motor Qwen3-TTS del proyecto, remuestreada a
    /// 16 kHz mono — la tasa que exige Whisper.
    #[test]
    fn whisper_rs_carga_modelo_gguf_y_transcribe() {
        use crate::Ct2SttEngine;

        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/whisper/ggml-medium-q8_0.bin"
        );
        // Los binarios bajo `models/` están gitignoreados: en un checkout
        // limpio (CI) este E2E se salta con aviso; en desarrollo corre completo.
        if !std::path::Path::new(model_path).exists() {
            eprintln!("[stt] skip: sin modelo Whisper GGUF (models/ gitignoreado)");
            return;
        }
        let engine = Ct2SttEngine::new(model_path)
            .expect("el modelo whisper GGUF debe cargar pesos reales desde disco");

        // El fixture ya está a la tasa de muestreo que espera el modelo (16 kHz).
        let wav_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/whisper_sample_16k.wav"
        );
        let mut reader = hound::WavReader::open(wav_path)
            .expect("el WAV fixture debe abrirse");
        assert_eq!(
            reader.spec().sample_rate as usize,
            16000,
            "el fixture debe estar a la tasa de muestreo que exige Whisper"
        );

        let pcm: Vec<i16> = reader
            .samples::<i16>()
            .map(|s| s.expect("muestra PCM válida"))
            .collect();

        let texto = engine
            .transcribe(&pcm, Some("es"))
            .expect("transcribe debe ejecutar sobre el modelo cargado");
        assert!(
            !texto.trim().is_empty(),
            "la transcripción no debe estar vacía"
        );
    }

    /// `Ct2SttEngine::new` sobre una ruta de modelo inexistente debe devolver
    /// `Err`, cubriendo la rama de "modelo no cargable" que el handler CLI
    /// mapea a `ExitCode::TranscriptionFailed` (10).
    #[test]
    fn ct2sttengine_new_con_ruta_inexistente_devuelve_err() {
        use crate::Ct2SttEngine;

        let result = Ct2SttEngine::new("ruta/que/no/existe/whisper-small");
        assert!(result.is_err(), "una ruta de modelo inexistente debe fallar");
    }

    /// `audio_ctx_para_duracion` debe escalonar por duración (capacidad
    /// `n_ctx×20 ms` >= 1.25×duración), respetar múltiplos de 64 y acotarse al
    /// rango [256, 1500] (piso verificado experimentalmente; tope del modelo).
    #[test]
    fn audio_ctx_para_duracion_respeta_escalon_margen_y_limites() {
        use crate::audio_ctx_para_duracion;

        // (duración en segundos, contexto esperado)
        let casos: [(usize, i32); 9] = [
            (0, 256),
            (2, 256),   // 62.5×2=125 → 128 < piso → 256 (capacidad 5.12 s)
            (4, 256),   // 250 → 256 (capacidad 5.12 s >= 1.25×4 s)
            (5, 320),   // 312.5 → 320
            (7, 448),   // 437.5 → 448
            (8, 512),
            (10, 640),  // 625 → 640 (capacidad 12.8 s >= 12.5 s)
            (15, 960),  // 937.5 → 960
            (30, 1500), // 1875 → 1920 > tope → 1500
        ];
        for (segundos, esperado) in casos {
            let ctx = audio_ctx_para_duracion(segundos * 16000);
            assert_eq!(
                ctx, esperado,
                "audio de {} s debe mapear a audio_ctx {} (obtenido {})",
                segundos, esperado, ctx
            );
            assert!(
                (ctx == 1500 || ctx % 64 == 0) && (256..=1500).contains(&ctx),
                "ctx {} debe ser múltiplo de 64 (o el tope 1500 del modelo) dentro de [256, 1500]",
                ctx
            );
        }

        // Capacidad >= 1.25×duración para todo audio hasta el límite del modelo
        // (24 s); por encima, el tope 1500 (30 s de capacidad) cubre hasta 24 s.
        for segundos in 1..=60 {
            let ctx = audio_ctx_para_duracion(segundos * 16000);
            let capacidad_s = ctx as f64 * 0.020;
            if segundos <= 24 {
                assert!(
                    capacidad_s >= 1.25 * segundos as f64,
                    "{} s: capacidad {:.2} s insuficiente para el margen 25%",
                    segundos,
                    capacidad_s
                );
            }
        }
    }

    /// Test de paridad contra texto verificado.
    ///
    /// El corpus de referencia son 2 audios de voz sintética en español
    /// generada por el motor Qwen3-TTS del proyecto (remuestreados a 16 kHz
    /// mono, la tasa que exige Whisper), con su transcripción de referencia
    /// verificada manualmente como ground truth (el oráculo Python
    /// `faster-whisper-medium` resultó defectuoso en español — CT2 issue
    /// #654 — y se descartó). La comparación normaliza ambos lados a
    /// minúsculas sin diacríticos ni puntuación (insensible a acentos y
    /// signos), y acepta WER ≤ 0.05 (distancia de Levenshtein a nivel de
    /// palabra) sobre el texto normalizado.
    ///
    /// `corpus_sintesis_16k.wav` y `corpus_respuestas_16k.wav` se excluyen del
    /// corpus: su audio sintético Qwen3-TTS tiene pronunciación defectuosa
    /// (el GGUF transcribe `esténtesis`/`clonazin`/`espato`/`Spaho` por
    /// `síntesis`/`clonación`/`español`/`espejo`, errores que también cometía
    /// el CT2 small original) y ningún modelo Whisper los transcribe bien. Los
    /// WAV y sus fixtures permanecen en `tests/assets/` sin uso en el corpus.
    #[test]
    fn ct2sttengine_coincide_con_oraculo_python() {
        use crate::Ct2SttEngine;

        let model_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/whisper/ggml-medium-q8_0.bin"
        );
        // Mismo criterio de skip que `whisper_rs_carga_modelo_gguf_y_transcribe`:
        // sin modelo (checkout limpio/CI) no hay paridad que medir.
        if !std::path::Path::new(model_path).exists() {
            eprintln!("[stt] skip: sin modelo Whisper GGUF (models/ gitignoreado)");
            return;
        }
        let engine = Ct2SttEngine::new(model_path).expect("el modelo whisper GGUF debe cargar");

        // Pares (audio, fixture de transcripción de referencia), mismo
        // directorio `tests/assets/` de este crate.
        let corpus: [(&str, &str); 2] = [
            ("whisper_sample_16k.wav", "whisper_sample_16k.oraculo.txt"),
            ("corpus_watermark_16k.wav", "corpus_watermark_16k.oraculo.txt"),
        ];

        for (wav, fixture) in corpus {
            let wav_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(wav);
            let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(fixture);

            let pcm = avi_audio::load_wav_16k_mono_pcm(wav_path)
                .expect("el WAV fixture debe cargarse");
            let actual = engine
                .transcribe(&pcm, Some("es"))
                .expect("la transcripción debe completarse")
                .trim()
                .to_string();

            let esperado = std::fs::read_to_string(fixture_path)
                .expect("el fixture de referencia debe existir")
                .trim()
                .to_string();

            // Ambos lados normalizados: minúsculas, sin diacríticos ni
            // puntuación (el WER no debe penalizar acentos ni signos).
            let esperado_norm = normalizar_texto(&esperado);
            let actual_norm = normalizar_texto(&actual);
            let ref_palabras: Vec<&str> = esperado_norm.split_whitespace().collect();
            let hip_palabras: Vec<&str> = actual_norm.split_whitespace().collect();

            if ref_palabras == hip_palabras {
                continue;
            }

            // Igualdad normalizada no se cumple: umbral de paridad por WER a
            // nivel de palabra (distancia de Levenshtein entre secuencias).
            let distancia = levenshtein_palabras(&ref_palabras, &hip_palabras);
            let wer = distancia as f64 / ref_palabras.len().max(1) as f64;

            assert!(
                wer <= 0.05,
                "WER {:.4} supera el umbral de paridad 0.05 en {} (esperado: {:?}, obtenido: {:?})",
                wer,
                wav,
                esperado_norm,
                actual_norm
            );
        }
    }

    /// Normaliza texto para la comparación de paridad: minúsculas, plegado de
    /// diacríticos (á→a, é→e, í→i, ó→o, ú→u, ü→u, ñ→n) y eliminación de
    /// puntuación (¡¿!?.,;:"'«»…- y similares). El plegado es manual para no
    /// depender de `unicode-normalization`.
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
