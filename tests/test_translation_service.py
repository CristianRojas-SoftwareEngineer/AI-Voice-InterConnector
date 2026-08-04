"""
Tests deterministas de `TranslationService`: orquesta el pipeline completo
(segmentar -> traducir -> ensamblar) con los cuatro colaboradores inyectados,
con atajo de passthrough cuando origen == destino, y una regression sobre un
corpus fijo es<->en usando dobles de prueba (nunca CT2/transformers reales).
"""

import pytest

from tts_sidecar.translation.service import TranslationService


class _ExplodingLoader:
    """Doble que falla si se invoca: usado para verificar que el passthrough
    nunca carga ningún modelo."""

    def load(self, cache_dir):
        raise AssertionError("no debería cargarse ningún modelo en passthrough")


class _ExplodingTranslator:
    """Doble que falla si se invoca: usado para verificar que el passthrough
    nunca traduce nada."""

    def translate(self, segment, source, target, cache_dir):
        raise AssertionError("no debería traducirse nada en passthrough")


def test_translate_passthrough_when_origin_equals_target():
    """origen == destino devuelve el texto intacto sin tocar loader/translator."""
    service = TranslationService(
        model_loader=_ExplodingLoader(),
        segmenter=None,
        translator=_ExplodingTranslator(),
        assembler=None,
    )

    result = service.translate("Hola, ¿cómo estás?", "es", "es")

    assert result == "Hola, ¿cómo estás?"


class _FakeLoader:
    """Doble de `TranslationModelLoader`: siempre "carga" con éxito y
    registra con qué `cache_dir` se le llamó."""

    def __init__(self):
        self.calls = []

    def load(self, cache_dir):
        self.calls.append(cache_dir)
        return "fake-model"


class _FakeSegmenter:
    def segment(self, text, language):
        return [[text]]


class _UppercaseTranslator:
    def translate(self, segment, source, target, cache_dir):
        return segment.upper()


class _FakeAssembler:
    def assemble(self, paragraphs):
        return "\n\n".join(" ".join(segments) for segments in paragraphs)


def test_translate_runs_full_pipeline_with_injected_collaborators():
    """origen != destino ejecuta segmentar -> traducir -> ensamblar."""
    loader = _FakeLoader()
    service = TranslationService(
        model_loader=loader,
        segmenter=_FakeSegmenter(),
        translator=_UppercaseTranslator(),
        assembler=_FakeAssembler(),
    )

    result = service.translate("hola mundo", "es", "en")

    assert result == "HOLA MUNDO"
    assert loader.calls  # el modelo se cargó (fail-fast) antes de traducir


# Corpus fijo es<->en con traducciones de referencia fijadas. El colaborador
# de traducción es un doble por diccionario: no invoca ningún runtime real,
# solo verifica que el servicio orquesta correctamente segmentación y
# ensamblado alrededor de una traducción por segmento ya conocida.
_CORPUS = [
    ("es", "en", "Hola.", "Hello."),
    ("en", "es", "Good morning.", "Buenos días."),
    (
        "es",
        "en",
        "Frase uno. Frase dos.",
        "Sentence one. Sentence two.",
    ),
]

_REFERENCE = {
    "Hola.": "Hello.",
    "Good morning.": "Buenos días.",
    "Frase uno.": "Sentence one.",
    "Frase dos.": "Sentence two.",
}


class _DictionaryTranslator:
    """Doble de traducción por diccionario fijo, sin runtime real."""

    def translate(self, segment, source, target, cache_dir):
        return _REFERENCE[segment]


class _PysbdLikeSegmenter:
    """Doble que fuerza partición en múltiples segmentos por oración, igual
    que produciría `SentenceSegmenter` real sobre estas frases cortas."""

    def segment(self, text, language):
        import re

        sentences = [s.strip() for s in re.split(r"(?<=[.])\s+", text) if s.strip()]
        return [sentences]


@pytest.mark.parametrize("source,target,text,expected", _CORPUS)
def test_translate_regression_over_fixed_corpus(source, target, text, expected):
    service = TranslationService(
        model_loader=_FakeLoader(),
        segmenter=_PysbdLikeSegmenter(),
        translator=_DictionaryTranslator(),
        assembler=_FakeAssembler(),
    )

    result = service.translate(text, source, target)

    assert result == expected
