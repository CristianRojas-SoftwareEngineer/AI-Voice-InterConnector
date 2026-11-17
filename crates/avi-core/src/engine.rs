// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

use crate::exit_codes::{CliError, ExitCode};

pub trait SttEngine: Send + Sync {
    fn transcribe(&self, audio_pcm: &[i16]) -> anyhow::Result<String>;
}

/// Única fuente de la regla de transcripción para la CLI y el daemon: transcribe
/// con el motor, trata el texto vacío como un error y da el error en el formato
/// común. Ningún otro sitio de producción llama a `SttEngine::transcribe`.
///
/// Devuelve:
/// - el texto tal como lo dio el motor, si tiene contenido;
/// - `empty_transcript` (código 2, sin detalle) si el texto queda vacío tras
///   `trim`, es decir, el motor no reconoció voz;
/// - `transcription_failed` (código 10) si el motor falla, con la cadena de
///   causas de la biblioteca solo en `detail`, nunca en el mensaje.
pub fn transcribe_speech(engine: &dyn SttEngine, pcm: &[i16]) -> Result<String, CliError> {
    let text = engine.transcribe(pcm).map_err(|e| {
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            "No se pudo transcribir el audio.",
        )
        .with_detail(format!("{e:#}"))
    })?;
    if text.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_transcript",
            "No se reconoció voz en el audio.",
        ));
    }
    Ok(text)
}

pub trait TranslationEngine: Send + Sync {
    fn translate(&self, text: &str) -> anyhow::Result<String>;
}

pub trait Segmenter: Send + Sync {
    fn segment(&self, text: &str) -> Vec<Vec<String>>;
}

/// Segmentador jerárquico de cuatro niveles (párrafo → oración → puntuación
/// fuerte → tokens): cada nivel solo se aplica a los fragmentos que aún exceden
/// `max_length`; el resto se deja intacto. Ningún nivel rompe una palabra ni
/// pierde texto.
///
/// El nivel de oración usa una regla determinista propia (escaneo manual de
/// caracteres). No maneja abreviaturas ni decimales (p. ej. "Sr.", "3.14"): es
/// una limitación conocida y aceptada, no un defecto a corregir.
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
/// de texto, conservando la puntuación terminal en el segmento.
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
/// (equivale a cortar con la regex `(?<=[,;:])\s+`). Si no hay nada que
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

/// Núcleos físicos del equipo, para dimensionar el paralelismo de los motores
/// (ONNX Runtime para el STT y CTranslate2 vía ct2rs para la traducción). Se
/// usan físicos y no lógicos a propósito: los hilos de estos motores hacen
/// busy-wait en las barreras de sincronización, y lanzar más hilos que núcleos
/// físicos (SMT/Hyper-Threading) sobre-suscribe las unidades SIMD y degrada el
/// throughput manteniendo el 100% de CPU. Los
/// motores usan los recursos del equipo del usuario, no una máquina de
/// desarrollo fija.
pub fn available_threads() -> usize {
    num_cpus::get_physical().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit_codes::ExitCode;

    /// Motor falso que devuelve siempre el resultado configurado.
    struct FakeStt(Result<&'static str, &'static str>);

    impl SttEngine for FakeStt {
        fn transcribe(&self, _audio_pcm: &[i16]) -> anyhow::Result<String> {
            match self.0 {
                Ok(text) => Ok(text.to_string()),
                Err(cause) => Err(anyhow::anyhow!(cause).context("fallo de la sesión")),
            }
        }
    }

    #[test]
    fn empty_or_blank_text_is_empty_transcript_without_detail() {
        for text in ["", "  \n"] {
            let err = transcribe_speech(&FakeStt(Ok(text)), &[0; 16]).unwrap_err();
            assert_eq!(err.code, ExitCode::InvalidInput);
            assert_eq!(err.reason, "empty_transcript");
            assert_eq!(err.message, "No se reconoció voz en el audio.");
            assert!(err.detail.is_none());
        }
    }

    #[test]
    fn engine_failure_is_transcription_failed_with_chained_cause_in_detail() {
        let err = transcribe_speech(&FakeStt(Err("causa interna")), &[0; 16]).unwrap_err();
        assert_eq!(err.code, ExitCode::TranscriptionFailed);
        assert_eq!(err.reason, "transcription_failed");
        assert_eq!(err.message, "No se pudo transcribir el audio.");
        let detail = err.detail.expect("el fallo del motor lleva detalle");
        assert!(detail.contains("causa interna"), "detalle: {detail}");
    }

    #[test]
    fn normal_text_passes_through_untouched() {
        let text = transcribe_speech(&FakeStt(Ok(" hola mundo ")), &[0; 16]).unwrap();
        assert_eq!(text, " hola mundo ");
    }
}
