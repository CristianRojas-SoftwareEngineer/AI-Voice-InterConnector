"""
Tests deterministas de `SentenceSegmenter`: segmentación jerárquica de cuatro
niveles (párrafo -> oración vía pysbd -> puntuación fuerte -> tokens) sin
perder texto ni romper el orden original.
"""

from ai_voice_interconnector.translation.segmenter import SentenceSegmenter


def test_short_text_returns_single_segment():
    """Un texto que cabe entero en un segmento se devuelve como único segmento."""
    segmenter = SentenceSegmenter(max_length=200)
    result = segmenter.segment("Hola, ¿cómo estás?", "es")
    assert result == [["Hola, ¿cómo estás?"]]


def test_multi_sentence_text_splits_by_sentence_via_pysbd_es():
    """Texto multi-oración que excede el límite se particiona en oraciones vía pysbd."""
    segmenter = SentenceSegmenter(max_length=30)
    text = "Hola mundo. Esta es la segunda oración. Y una tercera aquí."
    result = segmenter.segment(text, "es")

    assert len(result) == 1  # un solo párrafo
    sentences = result[0]
    assert len(sentences) == 3
    assert sentences[0].strip().startswith("Hola mundo")
    assert sentences[-1].strip().startswith("Y una tercera")


def test_multi_sentence_text_splits_by_sentence_via_pysbd_en():
    segmenter = SentenceSegmenter(max_length=35)
    text = "Hello world. This is the second sentence. And a third one here."
    result = segmenter.segment(text, "en")

    sentences = result[0]
    assert len(sentences) == 3
    assert sentences[0].strip().startswith("Hello world")


def test_preserves_paragraph_order():
    """Preserva el orden de párrafos y de las oraciones dentro de cada uno."""
    segmenter = SentenceSegmenter(max_length=10)
    text = "Uno. Dos.\n\nTres. Cuatro."
    result = segmenter.segment(text, "es")

    assert len(result) == 2
    assert result[0][0].strip().startswith("Uno")
    assert result[1][-1].strip().startswith("Cuatro")


def test_exceptionally_long_sentence_falls_back_to_strong_punctuation():
    """Una oración excepcionalmente larga cae al fallback de puntuación fuerte."""
    segmenter = SentenceSegmenter(max_length=25)
    # Una sola "oración" (sin punto final) con comas que exceden el límite.
    text = "Primero esto, luego lo otro, y finalmente esto de aquí"
    result = segmenter.segment(text, "es")

    sentences = result[0]
    assert len(sentences) > 1
    # Ningún segmento queda vacío ni se pierde texto: reensamblado bruto
    # (uniendo todo) debe seguir conteniendo cada fragmento original.
    joined = " ".join(sentences)
    assert "Primero esto" in joined
    assert "finalmente esto de aquí" in joined


def test_extreme_case_falls_back_to_tokenizer_without_losing_text():
    """Cuando ni la puntuación fuerte basta, cae al fallback por tokens (palabras)."""
    segmenter = SentenceSegmenter(max_length=10)
    text = "una palabra muy larga sin puntuacion que fuerza el ultimo recurso"
    result = segmenter.segment(text, "es")

    sentences = result[0]
    assert len(sentences) > 1
    for chunk in sentences:
        assert len(chunk) <= 10 or " " not in chunk.strip()
    joined = " ".join(sentences)
    for word in text.split(" "):
        assert word in joined
