"""Tests de los players de audio específicos de plataforma .

Windows (winsound) y macOS (afplay/subprocess) solo se ejercitaban de forma
indirecta o no se ejercitaban en absoluto: tests/test_audio.py cubre
SoundDevicePlayer (Linux) y get_audio_devices_with_status, pero no
WindowsAudioPlayer, MacOSAudioPlayer, ni el dispatch de AudioPlayer._init_*.

Este archivo corre en cualquier SO: winsound es real en Windows (el entorno de
CI/dev de este proyecto) y se mockea vía patch("winsound.PlaySound", ...); en
SO donde winsound no exista, se inyecta un módulo falso vía sys.modules para
poder ejercer WindowsAudioPlayer.play() sin depender de la plataforma real.
"""

import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from ai_voice_interconnector.audio import (
    AudioPlayer,
    MacOSAudioPlayer,
    SoundDevicePlayer,
    WindowsAudioPlayer,
    get_audio_devices_with_status,
)


def _fake_winsound():
    mock = MagicMock()
    mock.SND_FILENAME = 131072
    return mock


class TestWindowsAudioPlayer:
    def test_play_invokes_winsound_with_filename_flag(self):
        fake_winsound = _fake_winsound()
        with patch.dict(sys.modules, {"winsound": fake_winsound}):
            WindowsAudioPlayer().play(b"RIFF....WAVEfmt ")

        fake_winsound.PlaySound.assert_called_once()
        (path_arg, flag_arg), _ = fake_winsound.PlaySound.call_args
        assert path_arg.endswith(".wav")
        assert flag_arg == fake_winsound.SND_FILENAME

    def test_temp_file_is_removed_after_playing(self):
        import os

        captured_path = {}
        fake_winsound = _fake_winsound()

        def _capture(path, flag):
            captured_path["path"] = path
            assert os.path.exists(path)  # el archivo debe existir mientras se reproduce

        fake_winsound.PlaySound.side_effect = _capture
        with patch.dict(sys.modules, {"winsound": fake_winsound}):
            WindowsAudioPlayer().play(b"RIFF....WAVEfmt ")

        assert not os.path.exists(captured_path["path"])

    def test_temp_file_removed_even_when_playsound_raises(self):
        import os

        captured_path = {}
        fake_winsound = _fake_winsound()

        def _boom(path, flag):
            captured_path["path"] = path
            raise RuntimeError("winsound error")

        fake_winsound.PlaySound.side_effect = _boom
        with patch.dict(sys.modules, {"winsound": fake_winsound}):
            with pytest.raises(RuntimeError, match="winsound error"):
                WindowsAudioPlayer().play(b"RIFF....WAVEfmt ")

        assert not os.path.exists(captured_path["path"])

    def test_written_bytes_match_input(self):
        """El contenido escrito al tempfile es exactamente el WAV recibido."""
        written = {}
        fake_winsound = _fake_winsound()

        def _capture(path, flag):
            with open(path, "rb") as fh:
                written["data"] = fh.read()

        fake_winsound.PlaySound.side_effect = _capture
        payload = b"RIFF\x24\x00\x00\x00WAVEfmt some-fake-audio-bytes"
        with patch.dict(sys.modules, {"winsound": fake_winsound}):
            WindowsAudioPlayer().play(payload)

        assert written["data"] == payload


class TestMacOSAudioPlayer:
    def test_init_binds_subprocess_module(self):
        import subprocess

        player = MacOSAudioPlayer()
        assert player.subprocess is subprocess

    def test_play_invokes_afplay_with_temp_path(self):
        player = MacOSAudioPlayer()
        player.subprocess = MagicMock()
        player.play(b"RIFF....WAVEfmt ")

        player.subprocess.run.assert_called_once()
        (args,), kwargs = player.subprocess.run.call_args
        assert args[0] == "afplay"
        assert args[1].endswith(".wav")
        assert kwargs.get("check") is True

    def test_temp_file_removed_after_afplay(self):
        import os

        captured_path = {}

        def _capture(args, check=None):
            captured_path["path"] = args[1]
            assert os.path.exists(args[1])

        player = MacOSAudioPlayer()
        player.subprocess = MagicMock()
        player.subprocess.run.side_effect = _capture
        player.play(b"RIFF....WAVEfmt ")

        assert not os.path.exists(captured_path["path"])

    def test_temp_file_removed_even_if_afplay_fails(self):
        import os
        import subprocess as real_subprocess

        captured_path = {}

        def _boom(args, check=None):
            captured_path["path"] = args[1]
            raise real_subprocess.CalledProcessError(1, args)

        player = MacOSAudioPlayer()
        player.subprocess = MagicMock()
        player.subprocess.run.side_effect = _boom

        with pytest.raises(real_subprocess.CalledProcessError):
            player.play(b"RIFF....WAVEfmt ")

        assert not os.path.exists(captured_path["path"])


class TestAudioPlayerDispatch:
    @patch("platform.system", return_value="Windows")
    def test_dispatches_to_windows_player(self, _system):
        player = AudioPlayer()
        assert isinstance(player._player, WindowsAudioPlayer)

    @patch("platform.system", return_value="Darwin")
    def test_dispatches_to_macos_player(self, _system):
        player = AudioPlayer()
        assert isinstance(player._player, MacOSAudioPlayer)

    @patch("platform.system", return_value="Linux")
    def test_dispatches_to_sounddevice_player_on_linux(self, _system):
        fake_sd = MagicMock()
        with patch.dict(sys.modules, {"sounddevice": fake_sd}):
            player = AudioPlayer()
        assert isinstance(player._player, SoundDevicePlayer)
        assert player._player.sd is fake_sd

    @patch("platform.system", return_value="Linux")
    def test_linux_without_sounddevice_raises_import_error(self, _system):
        # Forzar que 'import sounddevice' falle: asignar None en sys.modules
        # hace que el import statement levante ImportError (mecanismo estándar).
        with patch.dict(sys.modules, {"sounddevice": None}):
            with pytest.raises(ImportError, match="sounddevice"):
                AudioPlayer()

    @patch("platform.system", return_value="Linux")
    def test_linux_without_portaudio_raises_actionable_oserror(self, _system):
        # sounddevice instalado pero PortAudio (libportaudio2) ausente: el import
        # de sounddevice levanta OSError al cargar la librería nativa vía CFFI.
        # Se simula interceptando __import__ para ese módulo; el mensaje debe
        # remitir a la instalación de libportaudio2, no a "instala sounddevice".
        import builtins

        real_import = builtins.__import__

        def _fake_import(name, *args, **kwargs):
            if name == "sounddevice":
                raise OSError("PortAudio library not found")
            return real_import(name, *args, **kwargs)

        with patch("builtins.__import__", side_effect=_fake_import):
            with pytest.raises(OSError, match="libportaudio2"):
                AudioPlayer()

    @patch("platform.system", return_value="Plan9")
    def test_unsupported_platform_raises_runtime_error(self, _system):
        with pytest.raises(RuntimeError, match="Plan9"):
            AudioPlayer()

    @patch("platform.system", return_value="Windows")
    def test_play_delegates_to_platform_player(self, _system):
        player = AudioPlayer()
        player._player = MagicMock()
        player.play(b"some-bytes")
        player._player.play.assert_called_once_with(b"some-bytes")

    @patch("platform.system", return_value="Windows")
    def test_play_file_reads_bytes_and_delegates(self, _system, tmp_path):
        wav_path = tmp_path / "sample.wav"
        wav_path.write_bytes(b"RIFF-fake-content")

        player = AudioPlayer()
        player._player = MagicMock()
        player.play_file(str(wav_path))

        player._player.play.assert_called_once_with(b"RIFF-fake-content")


class TestGetAudioDevicesMacOS:
    @patch("platform.system", return_value="Darwin")
    def test_sounddevice_failure_degrades_to_fallback_on_macos(self, _system):
        sd_mock = MagicMock()
        sd_mock.query_devices.side_effect = OSError("CoreAudio error")
        with patch.dict(sys.modules, {"sounddevice": sd_mock}):
            devices, degraded = get_audio_devices_with_status()
        assert degraded is True
        assert devices == [{"id": 0, "name": "Default", "latency": 0.1}]

    @patch("platform.system", return_value="Darwin")
    def test_sounddevice_success_lists_output_devices_only(self, _system):
        sd_mock = MagicMock()
        sd_mock.query_devices.return_value = [
            {"name": "Speakers", "max_output_channels": 2, "default_low_output_latency": 0.01},
            {"name": "Microphone", "max_output_channels": 0, "default_low_output_latency": 0.02},
        ]
        with patch.dict(sys.modules, {"sounddevice": sd_mock}):
            devices, degraded = get_audio_devices_with_status()
        assert degraded is False
        assert devices == [{"id": 0, "name": "Speakers", "latency": 0.01}]


class TestGetAudioDevicesUnknownPlatform:
    @patch("platform.system", return_value="Plan9")
    def test_unknown_platform_returns_degraded_fallback(self, _system):
        devices, degraded = get_audio_devices_with_status()
        assert degraded is True
        assert devices == [{"id": 0, "name": "Default", "latency": 0.1}]
