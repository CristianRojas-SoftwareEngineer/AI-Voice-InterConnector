"""
Tests deterministas de `SegmentAssembler`: reconstruye un texto único a
partir de los párrafos de segmentos traducidos (`list[list[str]]`, la misma
estructura que produce `SentenceSegmenter.segment`), preservando el orden y
los saltos de párrafo originales.
"""

from ai_voice_interconnector.translation.assembler import SegmentAssembler


def test_assembles_single_segment_without_alteration():
    """Un único segmento (texto corto, sin partición) se devuelve intacto."""
    assembler = SegmentAssembler()
    result = assembler.assemble([["Hello world"]])
    assert result == "Hello world"


def test_assembles_multiple_segments_preserving_order():
    """Varios segmentos dentro de un mismo párrafo se reensamblan en orden."""
    assembler = SegmentAssembler()
    result = assembler.assemble([["Hello world.", "This is the second sentence."]])
    assert result == "Hello world. This is the second sentence."


def test_assembles_multiple_paragraphs_preserving_paragraph_breaks():
    """Varios párrafos se reensamblan preservando el salto entre ellos."""
    assembler = SegmentAssembler()
    result = assembler.assemble([["Uno.", "Dos."], ["Tres.", "Cuatro."]])
    assert result == "Uno. Dos.\n\nTres. Cuatro."
