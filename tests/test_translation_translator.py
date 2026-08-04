"""
Tests deterministas de `MarianTranslator`: traduce un segmento usando el
modelo resuelto por `TranslationModelLoader`, con el backend de runtime
inyectable (nunca CT2 real en estos tests) y propagación de fallos con
identidad propia (`TranslationFailedError`).
"""

import pytest

from tts_sidecar.exceptions import TranslationFailedError
from tts_sidecar.translation.translator import MarianTranslator


class _FakeLoader:
    """Doble de `TranslationModelLoader`: devuelve un modelo fijo y registra
    con qué `cache_dir` se le llamó."""

    def __init__(self, model):
        self._model = model
        self.calls = []

    def load(self, cache_dir):
        self.calls.append(cache_dir)
        return self._model


def test_translate_segment_uses_injected_backend():
    """Traduce un segmento delegando en el backend inyectado sobre el modelo
    resuelto por el loader."""
    loader = _FakeLoader(model="fake-model")

    def fake_backend(model, segment):
        assert model == "fake-model"
        return segment.upper()

    translator = MarianTranslator(model_loader=loader, backend=fake_backend)
    result = translator.translate("hola mundo", "es", "en", "/cache/opus-mt-es-en")

    assert result == "HOLA MUNDO"


def test_translate_uses_loader_to_resolve_correct_model():
    """El `cache_dir` recibido se pasa intacto al loader inyectado."""
    loader = _FakeLoader(model="fake-model")
    translator = MarianTranslator(model_loader=loader, backend=lambda model, segment: segment)

    translator.translate("texto", "es", "en", "/cache/opus-mt-es-en")

    assert loader.calls == ["/cache/opus-mt-es-en"]


def test_translate_propagates_backend_failure_as_translation_failed_error():
    """Un fallo del backend de traducción se propaga con identidad propia,
    no como la excepción original del backend."""
    loader = _FakeLoader(model="fake-model")

    def failing_backend(model, segment):
        raise RuntimeError("el backend explotó")

    translator = MarianTranslator(model_loader=loader, backend=failing_backend)

    with pytest.raises(TranslationFailedError):
        translator.translate("texto", "es", "en", "/cache/opus-mt-es-en")
