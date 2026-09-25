//! Motor de traducción (Marian/opus-mt vía `ct2rs`).
//!
//! Expone `Ct2TranslationEngine`, implementación de
//! `avi_core::engine::TranslationEngine` que carga un modelo opus-mt convertido
//! a CT2 y traduce texto replicando la tokenización del oráculo Python
//! (`_MarianCT2Model.translate`): SentencePiece embebido + token `</s>` manual,
//! sin `sacremoses` ni `MarianTokenizer`.

// Estos símbolos solo los consume el motor real (`Ct2TranslationEngine` y
// `translate`), gateado tras `native-translation`; sin el feature quedarían sin
// uso, por eso el import se gatea junto con ellos.
#[cfg(feature = "native-translation")]
use avi_core::engine::{available_threads, HierarchicalSegmenter, Segmenter, TranslationEngine};
#[cfg(feature = "native-translation")]
use ct2rs::{ComputeType, Config, Translator};

/// Motor de traducción real sobre un modelo Marian/opus-mt en formato CT2.
#[cfg(feature = "native-translation")]
pub struct Ct2TranslationEngine {
    translator: Translator<ct2rs::tokenizers::auto::Tokenizer>,
}

#[cfg(feature = "native-translation")]
impl Ct2TranslationEngine {
    /// Carga el modelo CT2 ubicado en `model_dir`.
    pub fn new(model_dir: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let translator = Translator::new(
            model_dir,
            &Config {
                compute_type: ComputeType::INT8,
                // Hilos lógicos del equipo del usuario, no una máquina fija de
                // desarrollo (mismo criterio que el STT).
                num_threads_per_replica: available_threads(),
                ..Default::default()
            },
        )?;
        Ok(Self { translator })
    }

    /// Traduce un lote de oraciones en una única llamada a `translate_batch`,
    /// replicando el pre/post procesamiento por ítem de `translate`: anexa
    /// `" </s>"` al origen de cada oración, construye las opciones una sola
    /// vez y sanea cada hipótesis (token EOS final + espacios).
    fn translate_sentence_batch(
        &self,
        sentences: &[String],
        _source_lang: &str,
        _target_lang: &str,
    ) -> anyhow::Result<Vec<String>> {
        // El motor se instancia para una dirección fija según el `model_dir`
        // con el que se construyó; `source_lang`/`target_lang` no se usan aquí.
        // Se anexa `</s>` manualmente al origen: el encoder
        // Marian/opus-mt lo exige y `ct2-transformers-converter` no lo añade
        // automáticamente (ver nota técnica en el test
        // `ct2rs_loads_opus_mt_model_and_translates` de este mismo archivo).
        let sources: Vec<String> = sentences
            .iter()
            .map(|sentence| format!("{} </s>", sentence))
            .collect();
        // Mejora de calidad sobre el default de ct2rs: `disable_unk` suprime la
        // generación del token `<unk>` en la hipótesis (mismo default sano del
        // oráculo Python, `disable_unk=True` en ctranslate2), evitando `<unk>`
        // crudo en la salida ante vocabulario fuera de cobertura.
        let options = ct2rs::TranslationOptions {
            disable_unk: true,
            ..Default::default()
        };
        let results = self.translator.translate_batch(&sources, &options, None)?;
        if results.len() != sentences.len() {
            anyhow::bail!(
                "translate_batch devolvió {} resultados para {} oraciones",
                results.len(),
                sentences.len()
            );
        }
        Ok(results
            .into_iter()
            .map(|(translated, _)| {
                // La hipótesis del decoder termina con el token `</s>` (EOS),
                // que el detokenizador de ct2rs reconstruye como texto literal;
                // el oráculo lo elimina al decodificar con el SentencePiece
                // destino (los símbolos de control decodifican a cadena vacía,
                // `model_loader.py`). Se sanea aquí para preservar la paridad
                // de salida.
                translated.trim_end_matches("</s>").trim_end().to_string()
            })
            .collect())
    }
}

#[cfg(feature = "native-translation")]
impl TranslationEngine for Ct2TranslationEngine {
    fn translate(
        &self,
        text: &str,
        _source_lang: &str,
        _target_lang: &str,
    ) -> anyhow::Result<String> {
        // El texto único se traduce como lote de una sola oración: `translate_sentence_batch`
        // aplica el mismo pre/post procesamiento por ítem que el pipeline antiguo
        // (anexar `</s>`, opciones idénticas y saneo de la hipótesis).
        let mut hypothesis =
            self.translate_sentence_batch(&[text.to_string()], _source_lang, _target_lang)?;
        Ok(hypothesis.remove(0))
    }
}

/// Tope de oraciones por lote de traducción: un párrafo con más oraciones se
/// parte en grupos de `MAX_SENTENCES_PER_BATCH` para acotar la memoria y la
/// latencia de cada llamada a `translate_batch` (decisión cerrada:
/// acotar memoria y latencia por lote).
///
/// Lógica pura: se mantiene compilable/testeable sin el feature
/// `native-translation`; `allow(dead_code)` evita el warning-as-error cuando su
/// único consumidor de producción (`translate`) queda fuera del build.
#[cfg_attr(not(feature = "native-translation"), allow(dead_code))]
const MAX_SENTENCES_PER_BATCH: usize = 10;

/// Traduce los párrafos agrupando sus oraciones en lotes de a lo sumo
/// `MAX_SENTENCES_PER_BATCH`, una llamada a `translator` por lote, y devuelve el
/// mismo anidamiento de párrafos que la entrada. Las oraciones vacías (p. ej.
/// los párrafos generados por `"\n\n"` consecutivos) no se traducen pero el
/// párrafo conserva su posición como lista vacía para no alterar el
/// reensamblado posterior.
#[cfg_attr(not(feature = "native-translation"), allow(dead_code))]
#[allow(clippy::type_complexity)]
fn translate_batches_by_paragraph(
    paragraphs: Vec<Vec<String>>,
    source: &str,
    target: &str,
    translator: &dyn Fn(&[String], &str, &str) -> anyhow::Result<Vec<String>>,
) -> anyhow::Result<Vec<Vec<String>>> {
    let mut result = Vec::with_capacity(paragraphs.len());
    for paragraph in paragraphs {
        let sentences: Vec<String> = paragraph.into_iter().filter(|s| !s.is_empty()).collect();
        let mut translated = Vec::with_capacity(sentences.len());
        for batch in sentences.chunks(MAX_SENTENCES_PER_BATCH) {
            translated.extend(translator(batch, source, target)?);
        }
        result.push(translated);
    }
    Ok(result)
}

/// Traduce `text` de `source` a `target` segmentando jerárquicamente y
/// reensamblando el resultado igual que el oráculo (`SegmentAssembler`):
/// segmentos unidos con espacio dentro de cada párrafo y párrafos unidos con
/// `"\n\n"`. Precondición: `source != target` — el passthrough se resuelve en
/// la capa CLI antes de llamar a esta función.
#[cfg(feature = "native-translation")]
pub fn translate(
    text: &str,
    source: &str,
    target: &str,
    model_dir: impl AsRef<std::path::Path>,
) -> anyhow::Result<String> {
    let engine = Ct2TranslationEngine::new(model_dir)?;
    let segmenter = HierarchicalSegmenter::default();
    let paragraphs = segmenter.segment(text);

    // Cada párrafo se traduce en una sola llamada al motor (partida en grupos
    // de `MAX_SENTENCES_PER_BATCH` cuando excede el tope), en vez de una llamada
    // por oración: el reensamblado posterior es idéntico al anterior.
    let translated: Vec<Vec<String>> = translate_batches_by_paragraph(
        paragraphs,
        source,
        target,
        &|sentences: &[String], src: &str, dst: &str| {
            engine.translate_sentence_batch(sentences, src, dst)
        },
    )?;

    Ok(translated
        .into_iter()
        .map(|segments| segments.join(" "))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    // `HierarchicalSegmenter`/`Segmenter` los usan los tests puros de lote; el
    // trait `TranslationEngine` solo lo usan los tests del motor real (gateados).
    #[cfg(feature = "native-translation")]
    use avi_core::engine::TranslationEngine;
    use avi_core::engine::{HierarchicalSegmenter, Segmenter};

    /// Modelo CT2 derivado en HF cache `hf_cache_dir()/ct2` presente. Los snapshots
    /// y derivados están gitignoreados: en un checkout limpio (CI) los E2E se saltan.
    #[cfg(feature = "native-translation")]
    fn ct2_model_available(subdir: &str) -> bool {
        let pair = subdir.strip_prefix("opus-mt-").unwrap_or(subdir);
        avi_store::is_ct2_provisioned(pair)
    }

    /// Carga el modelo opus-mt es→en real (ya convertido a CT2 y provisionado)
    /// vía `Ct2TranslationEngine` y traduce un texto corto, verificando que el
    /// resultado no esté vacío.
    #[cfg(feature = "native-translation")]
    #[test]
    fn ct2translationengine_translates_real_text() {
        use crate::Ct2TranslationEngine;

        let model_dir = avi_store::ct2_model_dir("es-en");
        if !ct2_model_available("opus-mt-es-en") {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            return;
        }

        let engine = Ct2TranslationEngine::new(&model_dir)
            .expect("el modelo opus-mt-es-en debe cargar pesos CT2 reales desde disco");
        let translated = engine
            .translate("Hola, ¿cómo estás?", "es", "en")
            .expect("la traducción debe completarse");

        assert!(
            !translated.trim().is_empty(),
            "la traducción no debe estar vacía"
        );
        assert!(
            !translated.contains("</s>"),
            "el token EOS de origen no debe filtrarse a la salida"
        );
    }

    /// Carga un modelo CT2 real (opus-mt es→en, ya convertido y validado en
    /// disco) vía `Translator::new` y ejecuta una traducción corta, verificando
    /// que la salida no esté vacía. Reside aquí (y no en `avi-stt`) porque usa
    /// `ct2rs::Translator`, el runtime CT2 que este crate conserva.
    ///
    /// NOTA TÉCNICA: el encoder Marian/opus-mt exige el token `</s>` al final de
    /// la secuencia de origen. `ct2-transformers-converter` NO lo añade
    /// automáticamente (`config.json` del modelo trae `"add_source_eos": false`);
    /// sin ese token el decoder nunca converge a una traducción coherente. El
    /// motor real debe anexar `</s>` explícitamente al texto/tokens de origen
    /// antes de invocar `translate_batch` (o el tokenizador SentencePiece
    /// equivalente debe insertarlo).
    #[cfg(feature = "native-translation")]
    #[test]
    fn ct2rs_loads_opus_mt_model_and_translates() {
        use ct2rs::{Config, Translator};

        let model_dir = avi_store::ct2_model_dir("es-en");
        if !ct2_model_available("opus-mt-es-en") {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            return;
        }

        let translator = Translator::new(&model_dir, &Config::default())
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

    /// `Ct2TranslationEngine::new` sobre una ruta de modelo inexistente debe
    /// devolver `Err`, mismo patrón que
    /// `parakeet_engine_new_with_nonexistent_path_returns_err` de `avi-stt`.
    #[cfg(feature = "native-translation")]
    #[test]
    fn ct2translationengine_new_with_nonexistent_path_returns_err() {
        use crate::Ct2TranslationEngine;

        let result = Ct2TranslationEngine::new("ruta/que/no/existe/opus-mt-es-en");
        assert!(
            result.is_err(),
            "una ruta de modelo inexistente debe fallar"
        );
    }

    /// Regresión C-05/E1: un dir con solo `model.bin` (sin tokenizador) no es
    /// provisionado y el loader lo rechaza con `Err` — el gate coincide con el
    /// loader por construcción, sin depender de modelos reales.
    #[cfg(feature = "native-translation")]
    #[test]
    fn ct2_dir_with_only_model_bin_not_provisioned_and_loader_fails() {
        use crate::Ct2TranslationEngine;

        let dir = std::env::temp_dir().join(format!("avi_ct2_roto_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear dir temporal");
        std::fs::write(dir.join("model.bin"), b"senyuelo-sin-tokenizador")
            .expect("escribir model.bin señuelo");

        assert!(
            !avi_store::ct2_dir_missing_files(&dir).is_empty(),
            "un dir sin tokenizador no debe pasar el gate"
        );
        assert!(
            Ct2TranslationEngine::new(&dir).is_err(),
            "el loader debe rechazar un dir sin tokenizador"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Test de paridad funcional contra el oráculo Python.
    ///
    /// El corpus de referencia son pares `{input, expected}` generados con el
    /// pipeline de traducción del oráculo Python (`TranslationService` de
    /// producción, SentencePiece crudo + `</s>` manual) sobre textos reales
    /// del repositorio, en ambas direcciones es↔en.
    ///
    /// La paridad es FUNCIONAL, no byte a byte: la migración a Rust busca
    /// calidad y eficiencia, no clonar el comportamiento del oráculo (decisión
    /// del equipo). El corpus del oráculo se usa como referencia de CALIDAD,
    /// no como verdad esperada: sobre estos textos la varianza de paráfrasis
    /// entre dos hipótesis igualmente válidas alcanza WER 0.19 de media (p.
    /// ej. «Don't» vs «Do not», «a watermark» vs «any watermark», «Optimizar
    /// para la claridad externa» vs la forma conjugada del oráculo). Por eso
    /// los umbrales separan «variación válida» de «motor roto» (modelo
    /// equivocado, tokenización rota o salida degradada dispararían el WER
    /// medio muy por encima de 0.35), y se complementan con checks
    /// funcionales: salida no vacía, sin `</s>` ni `<unk>`.
    #[cfg(feature = "native-translation")]
    #[test]
    fn ct2translationengine_matches_python_oracle() {
        use crate::Ct2TranslationEngine;

        if !ct2_model_available("opus-mt-es-en") || !ct2_model_available("opus-mt-en-es") {
            eprintln!("[translate] skip: sin modelos CT2 es↔en");
            return;
        }

        // Pares (subdirectorio de modelo, fixture del corpus del oráculo),
        // ambos dentro de la raíz del crate.
        let corpus: [(&str, &str); 2] = [
            ("opus-mt-es-en", "translate_es_en.oraculo.json"),
            ("opus-mt-en-es", "translate_en_es.oraculo.json"),
        ];

        let mut wer_total = 0.0;
        let mut n_items = 0usize;

        for (model, fixture) in corpus {
            let pair = model.strip_prefix("opus-mt-").unwrap_or(model);
            let model_dir = avi_store::ct2_model_dir(pair);
            let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/assets")
                .join(fixture);

            let engine =
                Ct2TranslationEngine::new(model_dir).expect("el modelo opus-mt debe cargar");
            let oracle_pairs: Vec<OraclePair> = serde_json::from_str(
                &std::fs::read_to_string(fixture_path)
                    .expect("el corpus de referencia del oráculo debe existir"),
            )
            .expect("el corpus del oráculo debe ser JSON válido");

            for oracle_pair in &oracle_pairs {
                let actual = engine
                    .translate(&oracle_pair.input, "es", "en")
                    .expect("la traducción debe completarse")
                    .trim()
                    .to_string();

                assert!(
                    !actual.is_empty(),
                    "traducción vacía en {} para {:?}",
                    model,
                    oracle_pair.input
                );
                assert!(
                    !actual.contains("</s>"),
                    "el token EOS no debe filtrarse a la salida en {} para {:?}",
                    model,
                    oracle_pair.input
                );
                assert!(
                    !actual.contains("<unk>"),
                    "el token desconocido no debe filtrarse a la salida en {} para {:?}",
                    model,
                    oracle_pair.input
                );

                let expected = oracle_pair.expected.trim();
                let ref_words: Vec<&str> = expected.split_whitespace().collect();
                let hyp_words: Vec<&str> = actual.split_whitespace().collect();
                let distance = levenshtein_words(&ref_words, &hyp_words);
                let wer = distance as f64 / ref_words.len().max(1) as f64;

                assert!(
                    wer <= 0.6,
                    "WER por ítem {:.4} supera el tope 0.6 en {} ({:?}): esperado {:?}, obtenido {:?}",
                    wer,
                    model,
                    oracle_pair.input,
                    expected,
                    actual
                );

                wer_total += wer;
                n_items += 1;
                eprintln!(
                    "[corpus] {} | WER {:.4} | {:?} -> {:?}",
                    model, wer, oracle_pair.input, actual
                );
            }
        }

        let average_wer = wer_total / n_items.max(1) as f64;
        assert!(
            average_wer <= 0.35,
            "WER medio de corpus {:.4} supera el umbral 0.35 (motor degradado)",
            average_wer
        );
    }

    /// Un par del corpus de paridad: texto de entrada y su traducción de
    /// referencia emitida por el oráculo Python.
    #[cfg(feature = "native-translation")]
    #[derive(serde::Deserialize)]
    struct OraclePair {
        input: String,
        expected: String,
    }

    /// Distancia de Levenshtein a nivel de palabra entre `reference` e
    /// `hypothesis`, usada para calcular el WER de la prueba de paridad.
    #[cfg(feature = "native-translation")]
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

    /// End-to-end con el modelo real: un texto multi-párrafo (`"\n\n"` presente
    /// en la entrada) se traduce sin panic, con resultado no vacío y
    /// preservando la separación de párrafos en la salida.
    #[cfg(feature = "native-translation")]
    #[test]
    fn translate_multi_paragraph_preserves_separators() {
        if !ct2_model_available("opus-mt-es-en") {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            return;
        }
        let result = crate::translate(
            "Hola, ¿cómo estás?\n\nBuenos días, señor.",
            "es",
            "en",
            avi_store::ct2_model_dir("es-en"),
        );

        let translated = result.expect("la traducción multi-párrafo debe completarse");
        assert!(
            !translated.trim().is_empty(),
            "el resultado no debe estar vacío"
        );
        assert!(
            translated.contains("\n\n"),
            "la separación de párrafos debe preservarse en la salida"
        );
    }

    /// Cobertura acordada del exit 9 (`TranslationFailed`): un `model_dir`
    /// inexistente hace fallar el pipeline con `Err`
    /// (defensa en profundidad; no sustituye el chequeo `Path::exists` de la
    /// capa CLI).
    #[cfg(feature = "native-translation")]
    #[test]
    fn translate_with_nonexistent_model_dir_returns_err() {
        let result = crate::translate(
            "Hola, ¿cómo estás?",
            "es",
            "en",
            "ruta/que/no/existe/opus-mt-es-en",
        );
        assert!(result.is_err(), "un model_dir inexistente debe fallar");
    }

    /// Doble de traducción por lotes: `calls` cuenta las invocaciones y
    /// `sizes` acumula el número de oraciones de cada lote; las hipótesis se
    /// derivan como `"T:<oración>"`, lo que permite verificar partición, orden
    /// y reensamblado sin depender de ningún modelo.
    fn translator_double<'a>(
        calls: &'a Cell<usize>,
        sizes: &'a RefCell<Vec<usize>>,
    ) -> impl for<'x, 'y, 'z> Fn(&'x [String], &'y str, &'z str) -> anyhow::Result<Vec<String>> + 'a
    {
        move |batch: &[String], _source: &str, _target: &str| {
            calls.set(calls.get() + 1);
            sizes.borrow_mut().push(batch.len());
            Ok(batch
                .iter()
                .map(|sentence| format!("T:{}", sentence))
                .collect())
        }
    }

    /// Párrafo artificial de `n` oraciones distintas para las pruebas del lote.
    fn paragraph_of_n_sentences(n: usize) -> Vec<String> {
        (1..=n)
            .map(|i| format!("Oración número {} de la prueba.", i))
            .collect()
    }

    #[test]
    fn batch_translates_paragraph_of_5_sentences_in_one_call() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(5)],
            "es",
            "en",
            &double,
        )
        .expect("un párrafo de 5 oraciones no debe fallar");

        assert_eq!(
            calls.get(),
            1,
            "5 oraciones deben traducirse en una sola llamada"
        );
        assert_eq!(*sizes.borrow(), vec![5], "el lote debe tener 5 oraciones");
        assert_eq!(
            result,
            vec![(1..=5)
                .map(|i| format!("T:Oración número {} de la prueba.", i))
                .collect::<Vec<_>>()],
            "el orden de las oraciones debe preservarse"
        );
    }

    #[test]
    fn batch_splits_paragraph_of_11_sentences_into_2_calls() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(11)],
            "es",
            "en",
            &double,
        )
        .expect("un párrafo de 11 oraciones no debe fallar");

        assert_eq!(
            calls.get(),
            2,
            "11 oraciones deben partirse en 2 lotes por el tope de 10"
        );
        assert_eq!(
            *sizes.borrow(),
            vec![10, 1],
            "los lotes deben ser de 10 y 1"
        );
        assert_eq!(
            result,
            vec![(1..=11)
                .map(|i| format!("T:Oración número {} de la prueba.", i))
                .collect::<Vec<_>>()],
            "el orden global debe preservarse tras la partición"
        );
    }

    #[test]
    fn batch_splits_paragraph_of_20_sentences_into_2_calls() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(20)],
            "es",
            "en",
            &double,
        )
        .expect("un párrafo de 20 oraciones no debe fallar");

        assert_eq!(
            calls.get(),
            2,
            "20 oraciones deben partirse en 2 lotes exactos de 10"
        );
        assert_eq!(*sizes.borrow(), vec![10, 10]);
        assert_eq!(
            result,
            vec![(1..=20)
                .map(|i| format!("T:Oración número {} de la prueba.", i))
                .collect::<Vec<_>>()],
            "el orden global debe preservarse tras la partición"
        );
    }

    #[test]
    fn batch_text_of_one_sentence_makes_one_call() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(1)],
            "es",
            "en",
            &double,
        )
        .expect("un párrafo de 1 oración no debe fallar");

        assert_eq!(calls.get(), 1, "1 oración debe suponer 1 llamada");
        assert_eq!(*sizes.borrow(), vec![1]);
        assert_eq!(
            result,
            vec![vec!["T:Oración número 1 de la prueba.".to_string()]]
        );
    }

    #[test]
    fn batch_empty_text_does_not_invoke_translator() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        // `HierarchicalSegmenter` con `""` devuelve un párrafo con una única
        // oración vacía (comportamiento real observado del segmentador); la
        // oración vacía no debe invocar al traductor y el párrafo conserva su
        // posición para no alterar el reensamblado.
        let paragraphs = HierarchicalSegmenter::default().segment("");
        let result = super::translate_batches_by_paragraph(paragraphs, "es", "en", &double)
            .expect("un texto vacío no debe fallar");

        assert_eq!(
            calls.get(),
            0,
            "un texto vacío no debe invocar al traductor"
        );
        assert!(sizes.borrow().is_empty(), "no debe registrarse ningún lote");
        assert_eq!(
            result,
            vec![Vec::<String>::new()],
            "el párrafo vacío debe conservar su posición"
        );
    }

    #[test]
    fn batch_empty_paragraph_preserves_its_position_without_invoking() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        // Los `"\n\n"` consecutivos producen párrafos vacíos intermedios
        // (comportamiento real de `HierarchicalSegmenter`); no deben invocar al
        // traductor ni alterar el reensamblado posterior.
        let paragraphs = HierarchicalSegmenter::default().segment("Hola.\n\n\n\nAdiós.");
        let result = super::translate_batches_by_paragraph(paragraphs, "es", "en", &double)
            .expect("párrafos con huecos vacíos no deben fallar");

        assert_eq!(calls.get(), 2, "solo los párrafos no vacíos deben invocar");
        assert_eq!(
            result.len(),
            3,
            "los 3 párrafos deben conservar su posición"
        );
        assert!(result[1].is_empty(), "el párrafo vacío no debe traducirse");
    }

    #[test]
    fn batch_preserves_paragraph_and_sentence_order_in_multi_paragraph() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());
        let double = translator_double(&calls, &sizes);

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(5), paragraph_of_n_sentences(11)],
            "es",
            "en",
            &double,
        )
        .expect("el multipárrafo no debe fallar");

        assert_eq!(calls.get(), 3, "5 + 11 oraciones deben suponer 3 lotes");
        assert_eq!(*sizes.borrow(), vec![5, 10, 1], "lotes de 5, 10 y 1");
        assert_eq!(
            result,
            vec![
                (1..=5)
                    .map(|i| format!("T:Oración número {} de la prueba.", i))
                    .collect::<Vec<_>>(),
                (1..=11)
                    .map(|i| format!("T:Oración número {} de la prueba.", i))
                    .collect::<Vec<_>>(),
            ],
            "el orden global y por párrafo debe preservarse"
        );

        // Reensamblado equivalente al del pipeline: oraciones con `" "` y
        // párrafos con `"\n\n"`.
        let assembled: Vec<String> = result.iter().map(|p| p.join(" ")).collect();
        let text = assembled.join("\n\n");
        assert_eq!(
            text.matches("\n\n").count(),
            1,
            "debe haber un separador de párrafos"
        );
        assert!(
            text.starts_with("T:Oración número 1 de la prueba. T:Oración número 2"),
            "el primer párrafo debe encabezar el texto"
        );
        assert!(
            text.ends_with("T:Oración número 11 de la prueba."),
            "el último párrafo debe cerrar el texto"
        );
    }

    #[test]
    fn batch_propagates_translator_errors() {
        let calls = Cell::new(0usize);
        let sizes = RefCell::new(Vec::new());

        // Doble que falla en la segunda llamada: el error debe propagarse sin
        // pánico ni resultado parcial.
        let double = |batch: &[String], _source: &str, _target: &str| {
            calls.set(calls.get() + 1);
            sizes.borrow_mut().push(batch.len());
            if calls.get() == 2 {
                Err(anyhow::anyhow!("fallo deliberado de la segunda llamada"))
            } else {
                Ok(batch
                    .iter()
                    .map(|sentence| format!("T:{}", sentence))
                    .collect())
            }
        };

        let result = super::translate_batches_by_paragraph(
            vec![paragraph_of_n_sentences(5), paragraph_of_n_sentences(11)],
            "es",
            "en",
            &double,
        );

        assert!(result.is_err(), "el error del traductor debe propagarse");
        assert_eq!(
            calls.get(),
            2,
            "debe detenerse en la llamada que falla sin continuar"
        );
    }

    /// End-to-end con el modelo real (es→en): un párrafo de 11 oraciones
    /// (supera los 512 caracteres del segmentador, forzando la partición a
    /// oraciones) se traduce completo en dos lotes (10 + 1) sin perder
    /// contenido: salida no vacía, sin `</s>`/`<unk>` y con longitud acorde a
    /// la entrada (el corpus del oráculo solo cubre una oración por ítem, por
    /// eso esta cobertura del lote se añade aquí).
    #[cfg(feature = "native-translation")]
    #[test]
    fn translate_paragraph_of_11_sentences_splits_without_losing_text() {
        if !ct2_model_available("opus-mt-es-en") {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            return;
        }
        let sentences: Vec<String> = (1..=11)
            .map(|i| {
                format!(
                    "La reunión del día {} de la semana quedó programada para las diez \
                     de la mañana en la oficina central de la empresa.",
                    i
                )
            })
            .collect();
        let text = sentences.join(" ");

        let translated = crate::translate(&text, "es", "en", avi_store::ct2_model_dir("es-en"))
            .expect("el párrafo de 11 oraciones debe traducirse");

        assert!(
            !translated.trim().is_empty(),
            "la traducción no debe estar vacía"
        );
        assert!(
            !translated.contains("</s>"),
            "el token EOS no debe filtrarse a la salida"
        );
        assert!(
            !translated.contains("<unk>"),
            "el token desconocido no debe filtrarse a la salida"
        );
        // Sin pérdida de contenido: la salida conserva al menos la mitad de las
        // palabras de la entrada (umbral laxo que tolera la paráfrasis del
        // modelo pero dispararía ante la pérdida de oraciones completas).
        let input_words = text.split_whitespace().count();
        let output_words = translated.split_whitespace().count();
        assert!(
            output_words * 2 >= input_words,
            "la salida perdió contenido: {} palabras de entrada frente a {} de salida",
            input_words,
            output_words
        );
        // Y conserva la mayoría de las frases de la entrada (11 oraciones →
        // al menos 9 frases en la salida, tolerando fusiones del modelo).
        let output_phrases = translated
            .split(['.', '!', '?'])
            .filter(|f| !f.trim().is_empty())
            .count();
        assert!(
            output_phrases >= 9,
            "la salida debe conservar la mayoría de las 11 oraciones, obtuvo {} frases",
            output_phrases
        );
    }

    /// End-to-end con el modelo real (es→en): 3 párrafos (el central de 12
    /// oraciones, por encima de los 512 caracteres) se traducen preservando
    /// los dos separadores `"\n\n"` y sin filtrar `</s>`/`<unk>`.
    #[cfg(feature = "native-translation")]
    #[test]
    fn translate_long_multi_paragraph_preserves_paragraphs() {
        if !ct2_model_available("opus-mt-es-en") {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            return;
        }
        let sentences: Vec<String> = (1..=12)
            .map(|i| {
                format!(
                    "El equipo técnico del proyecto {} revisó los resultados \
                     de la última prueba durante toda la mañana.",
                    i
                )
            })
            .collect();
        let text = format!(
            "Hola, buenos días.\n\n{}\n\nHasta luego.",
            sentences.join(" ")
        );

        let translated = crate::translate(&text, "es", "en", avi_store::ct2_model_dir("es-en"))
            .expect("el multipárrafo largo debe traducirse");

        assert!(
            !translated.trim().is_empty(),
            "la traducción no debe estar vacía"
        );
        assert!(
            !translated.contains("</s>"),
            "el token EOS no debe filtrarse a la salida"
        );
        assert!(
            !translated.contains("<unk>"),
            "el token desconocido no debe filtrarse a la salida"
        );
        assert_eq!(
            translated.matches("\n\n").count(),
            2,
            "los dos separadores de párrafo deben preservarse"
        );
    }
}
