"""
Tests deterministas de `WhisperModelLoader` del subpaquete de transcripción.

El constructor del modelo (`model_factory`) es inyectable solo por
testabilidad: estos tests nunca instancian `faster_whisper.WhisperModel`
real, solo verifican la carga/caché en memoria.
"""

import pytest

from tts_sidecar.exceptions import TranscriptionModelMissingError
from tts_sidecar.transcription.model_loader import WhisperModelLoader


def test_load_returns_model_from_injected_factory(tmp_path):
    """Carga exitosa desde una ruta de caché simulada, vía el factory inyectado."""
    cache_dir = tmp_path / "faster-whisper-small"
    cache_dir.mkdir()
    calls = []

    def fake_factory(path):
        calls.append(path)
        return "fake-model"

    loader = WhisperModelLoader(model_factory=fake_factory)
    model = loader.load(cache_dir)

    assert model == "fake-model"
    assert calls == [str(cache_dir)]


def test_load_reuses_cached_instance(tmp_path):
    """Llamadas repetidas con la misma ruta no vuelven a invocar el factory."""
    cache_dir = tmp_path / "faster-whisper-small"
    cache_dir.mkdir()
    calls = []

    def fake_factory(path):
        calls.append(path)
        return object()

    loader = WhisperModelLoader(model_factory=fake_factory)
    first = loader.load(cache_dir)
    second = loader.load(cache_dir)

    assert first is second
    assert len(calls) == 1


def test_is_loaded_false_before_load_true_after(tmp_path):
    """`is_loaded` refleja el estado de la caché en memoria sin cargar nada."""
    cache_dir = tmp_path / "faster-whisper-small"
    cache_dir.mkdir()
    loader = WhisperModelLoader(model_factory=lambda path: "fake-model")

    assert loader.is_loaded(cache_dir) is False
    loader.load(cache_dir)
    assert loader.is_loaded(cache_dir) is True


def test_load_missing_cache_dir_raises():
    """Una ruta de caché inexistente eleva TranscriptionModelMissingError."""
    loader = WhisperModelLoader(model_factory=lambda path: "unused")

    with pytest.raises(TranscriptionModelMissingError):
        loader.load("/no/existe/faster-whisper-small")
