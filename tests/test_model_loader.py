"""
Tests deterministas de `ModelLoader`.

Cubren el routing es-mx-latam vs multilingüe y el invariante
`_require_voice_prompt`, sin cargar pesos reales ni disparar descargas de red:
los loaders internos y las dependencias de chatterbox/safetensors se sustituyen
por dobles vía `monkeypatch` sobre el namespace del módulo.
"""

from pathlib import Path

import pytest

from tts_sidecar.model_loader import ModelLoader


class _FakeComp:
    """Componente de modelo Chatterbox mínimo: acepta load/to/eval."""

    def load_state_dict(self, state, strict=False):
        return None

    def to(self, device):
        return self

    def eval(self):
        return self


class _FakeTTS:
    def __init__(self, *args, **kwargs):
        self.conds = kwargs.get("conds")

    def to(self, device):
        return self


def _stub_components(monkeypatch, ml):
    """Reemplaza las dependencias pesadas de chatterbox/safetensors por dobles."""
    monkeypatch.setattr(ml, "VoiceEncoder", lambda: _FakeComp())
    monkeypatch.setattr(ml, "T3", lambda *a, **k: _FakeComp())
    monkeypatch.setattr(
        ml, "T3Config", type("T3Config", (), {"multilingual": staticmethod(lambda: None)})
    )
    monkeypatch.setattr(ml, "S3Gen", lambda: _FakeComp())
    monkeypatch.setattr(ml, "MTLTokenizer", lambda *a, **k: _FakeComp())
    monkeypatch.setattr(ml, "ChatterboxMultilingualTTS", _FakeTTS)
    monkeypatch.setattr(ml, "load_file", lambda path: {"x": 1})


def _stub_download(monkeypatch, tmp_path):
    downloads = []

    def fake_hf_download(repo_id, filename, revision, token=None):
        downloads.append((repo_id, filename))
        p = tmp_path / filename
        p.write_bytes(b"ve")
        return str(p)

    monkeypatch.setattr("huggingface_hub.hf_hub_download", fake_hf_download)
    return downloads


def _make_es_latam_cache(tmp_path, has_conds=False):
    cache = (
        tmp_path
        / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        / "snapshots"
        / "abc"
    )
    cache.mkdir(parents=True)
    (cache / "t3_es_mx_latam.safetensors").write_bytes(b"x")
    (cache / "s3gen_v3.safetensors").write_bytes(b"x")
    (cache / "grapheme_mtl_merged_expanded_v1.json").write_bytes(b"{}")
    if has_conds:
        (cache / "conds.pt").write_bytes(b"conds")
    return cache


def test_load_routes_es_latam_vs_english_base(monkeypatch):
    """El routing replica engine.py:440-443 (subcadena 'es-mx-latam' en cache_dir)."""
    loader = ModelLoader()
    calls = {}

    def fake_es(cache_dir, compute_backend):
        calls["es"] = (cache_dir, compute_backend)
        return "es_tts"

    def fake_en(cache_dir, compute_backend):
        calls["en"] = (cache_dir, compute_backend)
        return "en_tts"

    monkeypatch.setattr(loader, "_load_es_latam", fake_es)
    monkeypatch.setattr(loader, "_load_english_base", fake_en)

    es_cache = "/cache/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam/snapshots/abc"
    assert loader.load(es_cache, "es-mx-latam", "cpu") == "es_tts"
    assert "es" in calls and "en" not in calls

    en_cache = "/cache/models--ResembleAI--chatterbox/snapshots/xyz"
    assert loader.load(en_cache, "en", "cpu") == "en_tts"
    assert "en" in calls and "es" in calls  # el de es ya se había registrado


def test_es_latam_without_ve_triggers_download(tmp_path, monkeypatch):
    """Sin ve.safetensors ni snapshot base, _load_es_latam descarga y exige voz."""
    import tts_sidecar.model_loader as ml

    cache = _make_es_latam_cache(tmp_path, has_conds=False)
    _stub_components(monkeypatch, ml)
    monkeypatch.setattr(ml, "_resolve_cached_snapshot", lambda *a, **k: None)
    downloads = _stub_download(monkeypatch, tmp_path)

    loader = ModelLoader()
    tts = loader._load_es_latam(cache, "cpu")

    assert downloads, "sin ve.safetensors debe disparar hf_hub_download"
    assert downloads[0] == ("ResembleAI/chatterbox", "ve.safetensors")
    assert tts._require_voice_prompt is True


def test_load_routes_en_cache_path_to_english_base_loader(monkeypatch):
    """Una ruta de caché del modelo inglés base (sin 'es-mx-latam') enruta a
    _load_english_base -> ChatterboxTTS.from_local (ruta de producción del
    modelo 'en' desde el rediseño cross-lingual, no un path muerto)."""
    loader = ModelLoader()
    calls = {}

    def fake_from_local(cache_dir, compute_backend):
        calls["from_local"] = (cache_dir, compute_backend)
        return "en_tts"

    monkeypatch.setattr(
        "tts_sidecar.model_loader.ChatterboxTTS.from_local", fake_from_local
    )

    en_cache = "/cache/models--ResembleAI--chatterbox/snapshots/abc"
    assert loader.load(en_cache, "en", "cpu") == "en_tts"
    assert calls["from_local"] == (Path(en_cache), "cpu")


def test_es_latam_with_conds_sets_require_prompt_false(tmp_path, monkeypatch):
    """Con conds.pt presente, el modelo no exige audio_prompt_path."""
    import tts_sidecar.model_loader as ml
    import chatterbox.mtl_tts as mtl

    cache = _make_es_latam_cache(tmp_path, has_conds=True)
    _stub_components(monkeypatch, ml)
    monkeypatch.setattr(ml, "_resolve_cached_snapshot", lambda *a, **k: None)
    _stub_download(monkeypatch, tmp_path)  # evita descarga real si falta ve

    class _FakeConds:
        @staticmethod
        def load(path, map_location=None):
            return _FakeTTS(conds=object())

    monkeypatch.setattr(mtl, "Conditionals", _FakeConds)

    loader = ModelLoader()
    tts = loader._load_es_latam(cache, "cpu")

    assert tts._require_voice_prompt is False
