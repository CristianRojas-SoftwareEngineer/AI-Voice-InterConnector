"""Tests del modelo de rutas uniforme (data_root, bundled_voices_dir).

Tras la reubicación de las voces de fábrica dentro del paquete, ambas
funciones deben resolver igual en los tres modos de ejecución: la fábrica
siempre relativa al paquete, los datos de usuario siempre al user-data-dir
por SO.
"""

import os
import sys

from tts_sidecar import paths, voices


class TestDataRoot:
    def test_returns_os_user_data_dir_on_linux(self, monkeypatch, tmp_path):
        monkeypatch.setattr(paths.sys, "platform", "linux")
        monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
        assert paths.data_root() == str(tmp_path / "tts-sidecar")

    def test_returns_os_user_data_dir_on_windows(self, monkeypatch, tmp_path):
        monkeypatch.setattr(paths.sys, "platform", "win32")
        monkeypatch.setenv("LOCALAPPDATA", str(tmp_path))
        assert paths.data_root() == str(tmp_path / "tts-sidecar")

    def test_creates_directory_if_missing(self, monkeypatch, tmp_path):
        monkeypatch.setattr(paths.sys, "platform", "linux")
        monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))
        root = paths.data_root()
        assert os.path.isdir(root)

    def test_does_not_depend_on_frozen_mode(self, monkeypatch, tmp_path):
        """El modelo uniforme no bifurca por is_frozen(): mismo resultado en ambos."""
        monkeypatch.setattr(paths.sys, "platform", "linux")
        monkeypatch.setenv("XDG_DATA_HOME", str(tmp_path))

        monkeypatch.delattr(paths.sys, "frozen", raising=False)
        no_frozen_root = paths.data_root()

        monkeypatch.setattr(paths.sys, "frozen", True, raising=False)
        frozen_root = paths.data_root()

        assert no_frozen_root == frozen_root


class TestBundledVoicesDir:
    def test_resolves_inside_the_package_when_not_frozen(self):
        monkeypatch_frozen = getattr(sys, "frozen", False)
        assert monkeypatch_frozen is False  # entorno de test normal: no congelado

        expected = os.path.join(
            os.path.dirname(os.path.abspath(paths.__file__)), "voices"
        )
        assert paths.bundled_voices_dir() == expected

    def test_default_voice_exists_with_both_audio_files(self):
        default_dir = os.path.join(paths.bundled_voices_dir(), "default")
        assert os.path.isfile(os.path.join(default_dir, "timbre-reference.wav"))
        assert os.path.isfile(os.path.join(default_dir, "speech-reference.wav"))

    def test_resolves_under_meipass_when_frozen(self, monkeypatch, tmp_path):
        monkeypatch.setattr(paths.sys, "frozen", True, raising=False)
        monkeypatch.setattr(paths.sys, "_MEIPASS", str(tmp_path), raising=False)

        assert paths.bundled_voices_dir() == os.path.join(
            str(tmp_path), "tts_sidecar", "voices"
        )


def test_default_voice_resolves_without_monkeypatching_roots():
    """La voz 'default' se resuelve directamente vía factory_voices_root(),
    sin necesidad de parchear rutas (a diferencia de los tests de voices.py
    que sí las parchean para aislar el registro de usuario)."""
    assert "default" in voices.list_voices()
