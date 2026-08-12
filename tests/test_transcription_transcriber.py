"""
Tests deterministas de `WhisperTranscriber`: delega en el backend inyectado
sobre el modelo resuelto por `model_loader`, y envuelve cualquier fallo del
backend en `TranscriptionFailedError`. Nunca toca faster-whisper real.
"""

import pytest

from ai_voice_interconnector.exceptions import TranscriptionFailedError
from ai_voice_interconnector.transcription.transcriber import WhisperTranscriber


class _FakeLoader:
    """Doble de `WhisperModelLoader`: siempre "carga" con éxito y registra
    con qué `cache_dir` se le llamó."""

    def __init__(self):
        self.calls = []

    def load(self, cache_dir):
        self.calls.append(cache_dir)
        return "fake-model"


def test_transcribe_delegates_to_backend_with_resolved_model():
    """El backend recibe el modelo resuelto por el loader, y el loader se
    invoca con el `cache_dir` correcto."""
    loader = _FakeLoader()
    captured = {}

    def fake_backend(model, audio, language):
        captured["model"] = model
        captured["audio"] = audio
        captured["language"] = language
        return "texto transcrito"

    transcriber = WhisperTranscriber(model_loader=loader, backend=fake_backend)

    result = transcriber.transcribe("audio-np-array", "es", "/ruta/cache")

    assert result == "texto transcrito"
    assert captured == {"model": "fake-model", "audio": "audio-np-array", "language": "es"}
    assert loader.calls == ["/ruta/cache"]


def test_transcribe_backend_failure_reraised_as_transcription_failed_error():
    """Un backend que levanta una excepción arbitraria se relanza como
    `TranscriptionFailedError`."""
    loader = _FakeLoader()

    def failing_backend(model, audio, language):
        raise RuntimeError("fallo simulado del backend")

    transcriber = WhisperTranscriber(model_loader=loader, backend=failing_backend)

    with pytest.raises(TranscriptionFailedError):
        transcriber.transcribe("audio-np-array", "es", "/ruta/cache")
