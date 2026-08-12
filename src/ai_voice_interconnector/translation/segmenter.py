"""
Segmentación jerárquica de texto para el traductor `opus-mt`.

MarianMT tiene un límite de entrada (~512 tokens): un texto largo no puede
pasarse tal cual, y un corte a media oración degrada la traducción. Esta
segmentación de cuatro niveles garantiza que ningún segmento entregado al
traductor exceda `max_length` sin romper una oración innecesariamente:

    párrafo -> oración (pysbd) -> puntuación fuerte -> tokens (último recurso)

Cada nivel solo se aplica al fragmento del nivel anterior que aún excede el
límite; el resto se deja intacto.
"""

import re

import pysbd

# Fallback de puntuación fuerte: separa por coma/punto y coma/dos puntos
# seguidos de espacio, cuando una oración completa (nivel pysbd) aún excede
# el límite.
_STRONG_PUNCTUATION = re.compile(r"(?<=[,;:])\s+")


class SentenceSegmenter:
    """Particiona un texto en segmentos que no exceden `max_length` caracteres,
    preservando párrafos y orden.

    El tokenizer del último nivel de fallback es inyectable solo por
    testabilidad/reutilización (p. ej. el tokenizer real de Marian, cuando el
    resto del pipeline lo tenga disponible); por defecto usa un tokenizer
    naive por espacios, suficiente para el caso extremo (oración sin
    puntuación que aún excede el límite).
    """

    def __init__(self, max_length: int = 512, tokenizer=None):
        self._max_length = max_length
        self._tokenizer = tokenizer
        self._pysbd_segmenters: dict[str, pysbd.Segmenter] = {}

    def segment(self, text: str, language: str) -> list[list[str]]:
        """Devuelve una lista de párrafos, cada uno una lista ordenada de
        segmentos (oraciones o fragmentos), ninguno más largo que `max_length`
        salvo que un solo token ya lo exceda (no se rompe una palabra)."""
        paragraphs = text.split("\n\n")
        return [self._segment_paragraph(p, language) for p in paragraphs]

    def _segment_paragraph(self, paragraph: str, language: str) -> list[str]:
        if len(paragraph) <= self._max_length:
            return [paragraph]

        segments = []
        for sentence in self._sentence_split(paragraph, language):
            if len(sentence) <= self._max_length:
                segments.append(sentence)
            else:
                segments.extend(self._punctuation_split(sentence))
        return segments

    def _sentence_split(self, text: str, language: str) -> list[str]:
        seg = self._pysbd_segmenters.get(language)
        if seg is None:
            seg = pysbd.Segmenter(language=language, clean=False)
            self._pysbd_segmenters[language] = seg
        return seg.segment(text)

    def _punctuation_split(self, sentence: str) -> list[str]:
        parts = [p for p in _STRONG_PUNCTUATION.split(sentence) if p]
        if len(parts) <= 1:
            # Sin puntuación fuerte que particionar: cae directo al tokenizer.
            return self._token_split(sentence)

        segments = []
        for part in parts:
            if len(part) <= self._max_length:
                segments.append(part)
            else:
                segments.extend(self._token_split(part))
        return segments

    def _token_split(self, text: str) -> list[str]:
        """Último recurso: agrupa tokens (palabras, o los del tokenizer
        inyectado) en chunks que no excedan `max_length`, sin perder texto."""
        tokens = self._tokenizer.tokenize(text) if self._tokenizer else text.split(" ")

        chunks = []
        current: list[str] = []
        current_len = 0
        for token in tokens:
            added_len = len(token) + (1 if current else 0)
            if current and current_len + added_len > self._max_length:
                chunks.append(" ".join(current))
                current = []
                current_len = 0
            current.append(token)
            current_len += len(token) + (1 if len(current) > 1 else 0)
        if current:
            chunks.append(" ".join(current))
        return chunks
