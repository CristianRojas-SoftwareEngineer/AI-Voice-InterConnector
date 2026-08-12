"""
Tests deterministas de `ConditionalsPreparer` (extracción de engine.py).

Cubren `compute` (ramas con/sin timbre_reference), `load_precomputed` (presente/
ausente) y `precompute_and_save`, sin el modelo real: se stubbean `librosa`,
`torch` y `chatterbox.mtl_tts` en `sys.modules`, y el cómputo corre contra un
`FakeTTS` que expone los atributos que lee `compute`.
"""

import os
import sys
import types

import numpy as np
import pytest

from ai_voice_interconnector.conditionals import ConditionalsPreparer


# ---------------------------------------------------------------------------
# Dobles (fakes)
# ---------------------------------------------------------------------------


class _FakeTensor:
    """Tensor mínimo: encadena las operaciones que usa `compute`."""

    def mean(self, axis=0, keepdim=False):
        return self

    def to(self, *args, **kwargs):
        return self

    def atleast_2d(self):
        return self

    def __rmul__(self, other):
        return self


class _FakeT3Cond:
    def __init__(self, **kwargs):
        self.kwargs = kwargs

    def to(self, **kwargs):
        return self


class _FakeConditionals:
    def __init__(self, t3_cond=None, s3gen_ref=None):
        self.t3_cond = t3_cond
        self.s3gen_ref = s3gen_ref

    @staticmethod
    def load(path, map_location=None):
        return _FakeConditionals()

    def to(self, device):
        return self

    def save(self, path):
        self.saved_to = path


class _FakeTTS:
    """Modelo Chatterbox mínimo: solo los atributos que lee `compute`."""

    ENC_COND_LEN = 10
    DEC_COND_LEN = 10

    def __init__(self):
        self.ve = types.SimpleNamespace(
            embeds_from_wavs=lambda wavs, sample_rate=None: np.zeros((1, 4))
        )
        self.s3gen = types.SimpleNamespace(
            embed_ref=lambda audio, sr, device: {"ref": audio},
            tokenizer=types.SimpleNamespace(
                forward=lambda toks, max_len: (np.zeros((1, 4)), None)
            ),
        )
        self.t3 = types.SimpleNamespace(
            hp=types.SimpleNamespace(speech_cond_prompt_len=6)
        )


def _install_fakes(monkeypatch, load_calls=None):
    """Instala en sys.modules los sustitutos de chatterbox/librosa/torch.

    `compute` hace `import librosa`, `import torch` y
    `from chatterbox.mtl_tts import Conditionals, T3Cond` de forma diferida, así
    que basta con sobreescribir las entradas de sys.modules durante el test.
    """
    load_calls = load_calls if load_calls is not None else []

    def fake_load(path, sr=None):
        load_calls.append(path)
        return (np.zeros(100), 24000)

    fake_librosa = types.SimpleNamespace(
        load=fake_load,
        resample=lambda y, orig_sr=None, target_sr=None: y,
    )

    fake_torch = types.SimpleNamespace(
        device=lambda *a, **k: object(),
        from_numpy=lambda a: _FakeTensor(),
        atleast_2d=lambda t: t if isinstance(t, _FakeTensor) else _FakeTensor(),
        ones=lambda *a, **k: _FakeTensor(),
    )

    fake_mtl = types.ModuleType("chatterbox.mtl_tts")
    fake_mtl.Conditionals = _FakeConditionals
    fake_mtl.T3Cond = _FakeT3Cond
    fake_chatterbox = types.ModuleType("chatterbox")
    fake_chatterbox.mtl_tts = fake_mtl

    monkeypatch.setitem(sys.modules, "chatterbox", fake_chatterbox)
    monkeypatch.setitem(sys.modules, "chatterbox.mtl_tts", fake_mtl)
    monkeypatch.setitem(sys.modules, "librosa", fake_librosa)
    monkeypatch.setitem(sys.modules, "torch", fake_torch)


# ---------------------------------------------------------------------------
# compute
# ---------------------------------------------------------------------------


@pytest.mark.parametrize("with_voice", [True, False])
def test_compute_with_and_without_timbre_reference(monkeypatch, with_voice):
    """Ejercita ambas ramas (engine.py:694-705) verificando la fuente de audio."""
    load_calls = []
    _install_fakes(monkeypatch, load_calls=load_calls)

    tts = _FakeTTS()
    prep = ConditionalsPreparer()
    speech = "speech-reference.wav"
    voice = "voice.wav" if with_voice else None

    result = prep.compute(tts, "cpu", voice, speech)

    assert isinstance(result, _FakeConditionals)
    assert result.s3gen_ref is not None and "ref" in result.s3gen_ref
    # Con timbre_reference se cargan ambos archivos; sin él, uno solo (se reusa).
    assert len(load_calls) == (2 if with_voice else 1)


def test_compute_returns_conditionals_with_prompt_tokens(monkeypatch):
    """El T3Cond resultante lleva los conditioning tokens y el speaker embedding."""
    _install_fakes(monkeypatch)
    tts = _FakeTTS()
    prep = ConditionalsPreparer()

    result = prep.compute(tts, "cpu", "voice.wav", "speech-reference.wav")

    assert isinstance(result.t3_cond, _FakeT3Cond)
    assert "cond_prompt_speech_tokens" in result.t3_cond.kwargs
    assert "speaker_emb" in result.t3_cond.kwargs


# ---------------------------------------------------------------------------
# load_precomputed
# ---------------------------------------------------------------------------


def test_load_precomputed_missing_returns_none(monkeypatch, tmp_path):
    _install_fakes(monkeypatch)
    prep = ConditionalsPreparer()
    assert prep.load_precomputed(str(tmp_path), "cpu") is None


def test_load_precomputed_present_returns_conds(monkeypatch, tmp_path):
    _install_fakes(monkeypatch)
    (tmp_path / "conditionals.pt").write_bytes(b"x")
    prep = ConditionalsPreparer()

    conds = prep.load_precomputed(str(tmp_path), "cpu")

    assert conds is not None
    assert isinstance(conds, _FakeConditionals)


# ---------------------------------------------------------------------------
# precompute_and_save
# ---------------------------------------------------------------------------


def test_precompute_and_save_calls_compute_and_save(monkeypatch, tmp_path):
    _install_fakes(monkeypatch)
    saved = []
    monkeypatch.setattr(_FakeConditionals, "save", lambda self, path: saved.append(path))

    tts = _FakeTTS()
    prep = ConditionalsPreparer()
    voice_dir = str(tmp_path)

    prep.precompute_and_save(voice_dir, "timbre-reference.wav", "speech-reference.wav", tts, "cpu")

    expected = os.path.join(voice_dir, "conditionals.pt")
    assert saved == [expected]
