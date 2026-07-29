"""Tests de `ChatterboxEngine` a nivel voz.

Cubren `precompute_voice` y los delegates
`list_voices`/`remove_voice`/`resolve_voice`, inyectando colaboradores falsos
y monkeypatcheando el módulo `voices`, sin cargar el modelo real. El engine se
instancia con `ChatterboxEngine.__new__` (patrón ya usado en test_engine_cache.py).
"""

import pytest

import tts_sidecar.voices as voices_mod
from tts_sidecar.conditionals import ConditionalsPreparer
from tts_sidecar.engine import ChatterboxEngine
from tts_sidecar.model_loader import ModelLoader


class _FakePreparer:
    """Double de ConditionalsPreparer que registra las llamadas de precomputación."""

    def __init__(self):
        self.precompute_calls = []
        self.load_calls = []

    def compute(self, *args, **kwargs):
        return object()

    def precompute_and_save(self, voice_dir, timbre_reference, speech_reference, tts, compute_backend):
        self.precompute_calls.append(
            (voice_dir, timbre_reference, speech_reference, tts, compute_backend)
        )

    def load_precomputed(self, voice_dir, compute_backend):
        self.load_calls.append((voice_dir, compute_backend))
        return None


@pytest.fixture
def engine(monkeypatch):
    """Engine sin modelo real: colaboradores inyectados y `voice_paths` stubbeado."""
    eng = ChatterboxEngine.__new__(ChatterboxEngine)
    eng.compute_backend = "cpu"
    eng.model_name = "es-mx-latam"
    eng._model_loader = ModelLoader()  # no se usa en estos tests
    eng._conditionals_prep = ConditionalsPreparer()
    eng._tts = object()

    def fake_voice_paths(name):
        return (f"/voices/{name}/timbre-reference.wav", f"/voices/{name}/speech-reference.wav")

    monkeypatch.setattr(voices_mod, "voice_paths", fake_voice_paths)
    return eng


def test_precompute_voice_invokes_preparer_with_registry_paths(engine):
    engine._conditionals_prep = _FakePreparer()

    engine.precompute_voice("v")

    assert engine._conditionals_prep.precompute_calls == [
        ("/voices/v", "/voices/v/timbre-reference.wav", "/voices/v/speech-reference.wav", engine._tts, "cpu")
    ]


def test_precompute_voice_propagates_unknown_voice(engine, monkeypatch):
    def raise_not_found(name):
        raise FileNotFoundError(f"Voz '{name}' no encontrada")

    monkeypatch.setattr(voices_mod, "voice_paths", raise_not_found)
    engine._conditionals_prep = _FakePreparer()

    with pytest.raises(FileNotFoundError):
        engine.precompute_voice("missing")

    assert engine._conditionals_prep.precompute_calls == []


def test_list_voices_delegates(monkeypatch):
    eng = ChatterboxEngine.__new__(ChatterboxEngine)
    called = []
    monkeypatch.setattr(
        voices_mod, "list_voices", lambda: (called.append(1) or ["a", "b"])
    )

    assert eng.list_voices() == ["a", "b"]
    assert called == [1]


def test_remove_voice_delegates(monkeypatch):
    eng = ChatterboxEngine.__new__(ChatterboxEngine)
    names = []
    monkeypatch.setattr(
        voices_mod, "remove_voice", lambda name: (names.append(name) or True)
    )

    assert eng.remove_voice("foo") is True
    assert names == ["foo"]


def test_resolve_voice_delegates(monkeypatch):
    eng = ChatterboxEngine.__new__(ChatterboxEngine)
    names = []
    monkeypatch.setattr(
        voices_mod,
        "voice_paths",
        lambda name: (names.append(name) or ("r.wav", "s.wav")),
    )

    assert eng.resolve_voice("bar") == ("r.wav", "s.wav")
    assert names == ["bar"]
