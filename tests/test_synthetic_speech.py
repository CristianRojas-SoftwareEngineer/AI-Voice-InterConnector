"""Tests del almacén de habla sintética (`synthetic_speech`)."""

import json
import os

import pytest

from tts_sidecar import synthetic_speech


@pytest.fixture
def store(tmp_path, monkeypatch):
    """Aísla el almacén: `data_root()` apunta a un tmp_path por test."""
    root = tmp_path / "data"
    root.mkdir()
    monkeypatch.setattr(synthetic_speech.paths, "data_root", lambda: str(root))
    return root


class TestSave:
    def test_persists_wav_and_sidecar(self, store):
        wav = synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        assert os.path.exists(wav)
        sidecar = wav[: -len(".wav")] + ".json"
        assert os.path.exists(sidecar)
        with open(sidecar, encoding="utf-8") as f:
            meta = json.load(f)
        assert meta["voice"] == "default"
        assert meta["label"] == "saludo"
        assert meta["text"] == "Hola"
        assert meta["created_at"]  # ISO 8601, no vacío

    def test_writes_only_inside_store_root(self, store):
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        expected_root = store / "synthetic-speech"
        # Todo lo escrito bajo data_root vive dentro de synthetic-speech/.
        written = [p for p in store.rglob("*") if p.is_file()]
        assert written, "no se escribió nada"
        for path in written:
            assert expected_root in path.parents

    def test_label_normalized_to_lowercase(self, store):
        synthetic_speech.save("default", "Saludo", b"RIFFdata", "Hola")
        assert synthetic_speech.exists("default", "saludo")
        assert synthetic_speech.exists("default", "SALUDO")  # se normaliza al consultar

    def test_sidecar_written_before_wav(self, store, monkeypatch):
        """El WAV presente implica sidecar presente: fallar al escribir el WAV
        no debe dejar una toma reproducible sin metadato."""
        real_atomic = synthetic_speech._atomic_write
        calls = []

        def spy(path, data):
            calls.append(path)
            if path.endswith(".wav"):
                raise RuntimeError("fallo simulado al publicar el WAV")
            real_atomic(path, data)

        monkeypatch.setattr(synthetic_speech, "_atomic_write", spy)
        with pytest.raises(RuntimeError):
            synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        assert calls[0].endswith(".json") and calls[1].endswith(".wav")
        assert not synthetic_speech.exists("default", "saludo")


class TestExistsAndCollision:
    def test_collision_decided_by_wav(self, store):
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        assert synthetic_speech.exists("default", "saludo")
        assert not synthetic_speech.exists("default", "otra")

    def test_wav_path_raises_when_absent(self, store):
        with pytest.raises(FileNotFoundError):
            synthetic_speech.wav_path("default", "ausente")


class TestRemove:
    def test_removes_both_files(self, store):
        wav = synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        assert synthetic_speech.remove("default", "saludo") is True
        assert not os.path.exists(wav)
        assert not os.path.exists(wav[: -len(".wav")] + ".json")

    def test_returns_false_when_nothing_existed(self, store):
        assert synthetic_speech.remove("default", "ausente") is False

    def test_removes_orphan_sidecar(self, store):
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        wav = synthetic_speech.wav_path("default", "saludo")
        os.unlink(wav)  # queda el sidecar huérfano
        assert synthetic_speech.remove("default", "saludo") is True


class TestListEntries:
    def test_empty_when_store_absent(self, store):
        assert synthetic_speech.list_entries() == []

    def test_lists_by_wav_tolerating_missing_sidecar(self, store):
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        sidecar = synthetic_speech.wav_path("default", "saludo")[: -len(".wav")] + ".json"
        os.unlink(sidecar)  # sidecar ausente: la entrada sigue enumerándose por el WAV
        entries = synthetic_speech.list_entries()
        assert len(entries) == 1
        assert entries[0]["voice"] == "default"
        assert entries[0]["label"] == "saludo"
        assert entries[0]["text"] is None
        assert entries[0]["created_at"] is None

    def test_filters_by_voice(self, store):
        synthetic_speech.save("default", "a", b"RIFF", "A")
        synthetic_speech.save("otra", "b", b"RIFF", "B")
        entries = synthetic_speech.list_entries(voice="otra")
        assert [e["label"] for e in entries] == ["b"]

    def test_deterministic_order(self, store):
        synthetic_speech.save("zeta", "b", b"RIFF", "")
        synthetic_speech.save("alfa", "y", b"RIFF", "")
        synthetic_speech.save("alfa", "x", b"RIFF", "")
        entries = synthetic_speech.list_entries()
        assert [(e["voice"], e["label"]) for e in entries] == [
            ("alfa", "x"),
            ("alfa", "y"),
            ("zeta", "b"),
        ]


@pytest.mark.parametrize("bad", ["..", ".", "a/b", "a\\b", "", "voz/../x"])
def test_rejects_malicious_segments(store, bad):
    with pytest.raises(ValueError):
        synthetic_speech.save(bad, "ok", b"RIFF", "")
    with pytest.raises(ValueError):
        synthetic_speech.save("default", bad, b"RIFF", "")
