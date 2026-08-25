pub trait SttEngine: Send + Sync {
    fn transcribe(&self, audio_pcm: &[i16], language: Option<&str>) -> anyhow::Result<String>;
}

pub trait TranslationEngine: Send + Sync {
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str)
        -> anyhow::Result<String>;
}

pub trait Segmenter: Send + Sync {
    fn segment(&self, text: &str) -> Vec<Vec<String>>;
}

/// Segmentador jerárquico de cuatro niveles (párrafo → oración → puntuación
/// fuerte → tokens), fiel a la estructura de `SentenceSegmenter` del oráculo
/// Python: cada nivel solo se aplica a los fragmentos que aún exceden
/// `max_length`; el resto se deja intacto. Ningún nivel rompe una palabra ni
/// pierde texto.
///
/// El nivel de oración usa una regla determinista propia (escaneo manual de
/// caracteres) en vez de `pysbd`: no maneja abreviaturas ni decimales
/// (p. ej. "Sr.", "3.14"), brecha de fidelidad aceptada en el gate de plan
/// (ver F3-plan-refinado.md, Consideraciones fundamentales).
pub struct HierarchicalSegmenter {
    max_length: usize,
}

impl HierarchicalSegmenter {
    pub fn new(max_length: usize) -> Self {
        Self { max_length }
    }
}

impl Default for HierarchicalSegmenter {
    fn default() -> Self {
        Self::new(512)
    }
}

impl Segmenter for HierarchicalSegmenter {
    fn segment(&self, text: &str) -> Vec<Vec<String>> {
        text.split("\n\n")
            .map(|p| self.segment_paragraph(p))
            .collect()
    }
}

impl HierarchicalSegmenter {
    /// Nivel 1→2: si el párrafo cabe en `max_length` se devuelve entero; si no,
    /// se particiona por oraciones y cada oración que aún exceda el límite cae
    /// al nivel de puntuación fuerte (replica `_segment_paragraph`).
    fn segment_paragraph(&self, paragraph: &str) -> Vec<String> {
        if paragraph.chars().count() <= self.max_length {
            return vec![paragraph.to_string()];
        }
        let mut segments = Vec::new();
        for sentence in split_sentences(paragraph) {
            if sentence.chars().count() <= self.max_length {
                segments.push(sentence);
            } else {
                segments.extend(split_strong_punctuation(&sentence, self.max_length));
            }
        }
        segments
    }
}

/// Nivel 2 (oración, regla determinista propia): escaneo manual de caracteres
/// que corta tras una racha de `.`/`!`/`?` seguida de espacio en blanco o fin
/// de texto, conservando la puntuación terminal en el segmento (a diferencia
/// del `DeterministicSegmenter` naive anterior, que la descartaba).
fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    let mut pending = false;
    for c in text.chars() {
        if pending && c.is_whitespace() {
            sentences.push(current.trim().to_string());
            current = String::new();
            pending = false;
            continue;
        }
        current.push(c);
        pending = c == '.' || c == '!' || c == '?';
    }
    if !current.trim().is_empty() {
        sentences.push(current.trim().to_string());
    }
    sentences
}

/// Nivel 3 (puntuación fuerte): separa tras cada `,`/`;`/`:` seguida de
/// espacio en blanco, conservando la puntuación en el fragmento precedente
/// (equivalente al lookbehind `(?<=[,;:])\s+` de Python). Si no hay nada que
/// particionar, delega directo a `split_tokens`; si no, cada parte que aún
/// exceda `max_length` cae al nivel de tokens y el resto se deja intacto.
fn split_strong_punctuation(text: &str, max_length: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut prev_is_punct = false;
    for c in text.chars() {
        if prev_is_punct && c.is_whitespace() {
            parts.push(current.trim().to_string());
            current = String::new();
            prev_is_punct = false;
            continue;
        }
        current.push(c);
        prev_is_punct = c == ',' || c == ';' || c == ':';
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    if parts.len() <= 1 {
        return split_tokens(text, max_length);
    }
    let mut segments = Vec::new();
    for part in parts {
        if part.chars().count() <= max_length {
            segments.push(part);
        } else {
            segments.extend(split_tokens(&part, max_length));
        }
    }
    segments
}

/// Nivel 4 (tokens, último recurso): separa por espacio simple y empaqueta
/// tokens consecutivos en fragmentos unidos por `' '` que no excedan
/// `max_length`, sin partir ningún token individual (replica `_token_split`).
fn split_tokens(text: &str, max_length: usize) -> Vec<String> {
    let tokens: Vec<&str> = text.split(' ').collect();
    let mut chunks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut current_len = 0;
    for token in tokens {
        let token_len = token.chars().count();
        let added_len = token_len + if current.is_empty() { 0 } else { 1 };
        if !current.is_empty() && current_len + added_len > max_length {
            chunks.push(current.join(" "));
            current = Vec::new();
            current_len = 0;
        }
        current.push(token);
        current_len += token_len + if current.len() > 1 { 1 } else { 0 };
    }
    if !current.is_empty() {
        chunks.push(current.join(" "));
    }
    chunks
}

pub struct DummySttEngine;

impl SttEngine for DummySttEngine {
    fn transcribe(&self, _audio_pcm: &[i16], _language: Option<&str>) -> anyhow::Result<String> {
        Ok("Transcripción de prueba".to_string())
    }
}

pub struct DummyTranslationEngine;

impl TranslationEngine for DummyTranslationEngine {
    fn translate(
        &self,
        text: &str,
        source_lang: &str,
        target_lang: &str,
    ) -> anyhow::Result<String> {
        if source_lang == target_lang {
            return Ok(text.to_string());
        }
        Ok(format!("[{}->{}] {}", source_lang, target_lang, text))
    }
}

/// Núcleos físicos del equipo, para dimensionar el paralelismo de los motores
/// (whisper.cpp/ggml y ct2rs). Se usan físicos y no lógicos a propósito: los
/// hilos de ggml hacen busy-wait en las barreras de sincronización, y lanzar
/// más hilos que núcleos físicos (SMT/Hyper-Threading) sobre-suscribe las
/// unidades SIMD y degrada el throughput manteniendo el 100% de CPU. Los
/// motores usan los recursos del equipo del usuario, no una máquina de
/// desarrollo fija.
pub fn hilos_disponibles() -> usize {
    num_cpus::get_physical().max(1)
}
