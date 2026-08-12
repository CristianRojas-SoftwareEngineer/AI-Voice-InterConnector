"""Tests de la capa de reproducción y enumeración de audio."""

import base64
import io
import sys
import wave
from pathlib import Path
from unittest.mock import MagicMock, patch

import numpy as np

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

import pytest

from ai_voice_interconnector.audio import (
    INT16_MAX_F,
    AudioRecorder,
    SoundDevicePlayer,
    encode_pcm_int16_b64,
    get_audio_devices,
    get_audio_devices_with_status,
)


def _wav_bytes(n_channels: int, n_frames: int = 480, sample_rate: int = 24000) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, "wb") as wf:
        wf.setnchannels(n_channels)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        frames = np.zeros(n_frames * n_channels, dtype=np.int16)
        wf.writeframes(frames.tobytes())
    return buffer.getvalue()


class TestSoundDevicePlayer:
    def test_mono_plays_flat(self):
        sd = MagicMock()
        SoundDevicePlayer(sd).play(_wav_bytes(n_channels=1))
        (audio_np,), kwargs = sd.play.call_args
        assert audio_np.ndim == 1
        assert kwargs["samplerate"] == 24000

    def test_stereo_plays_with_two_channels(self):
        """Sin el reshape, un WAV estéreo sonaría como mono al doble de velocidad."""
        sd = MagicMock()
        SoundDevicePlayer(sd).play(_wav_bytes(n_channels=2, n_frames=480))
        (audio_np,), _ = sd.play.call_args
        assert audio_np.shape == (480, 2)

    def test_rejects_sample_width_other_than_16_bits(self):
        buffer = io.BytesIO()
        with wave.open(buffer, "wb") as wf:
            wf.setnchannels(1)
            wf.setsampwidth(3)  # 24 bits, no soportado
            wf.setframerate(24000)
            wf.writeframes(b"\x00" * 3 * 480)
        with pytest.raises(ValueError, match="ancho de muestra"):
            SoundDevicePlayer(MagicMock()).play(buffer.getvalue())


class TestGetAudioDevicesWindows:
    @patch("platform.system", return_value="Windows")
    def test_pycaw_failure_degrades_to_fallback(self, _system):
        """Un fallo COM (RDP, host sin audio) no debe crashear 'devices'."""
        pycaw_mock = MagicMock()
        pycaw_mock.pycaw.AudioUtilities.GetDeviceEnumerator.side_effect = OSError("COM error")
        with patch.dict(sys.modules, {"pycaw": pycaw_mock, "pycaw.pycaw": pycaw_mock.pycaw}):
            devices = get_audio_devices()
        assert devices == [{"id": 0, "name": "Default", "latency": 0.1}]


class TestGetAudioDevicesLinuxMacOS:
    @patch("platform.system", return_value="Linux")
    def test_non_import_error_failure_degrades_to_fallback(self, _system, caplog):
        """Un PortAudioError en tiempo de enumeración no debe crashear 'devices'.

        Además de degradar, el fallo queda registrado a nivel debug con
        traza, en vez de tragarse en silencio.
        """
        import logging

        sd_mock = MagicMock()
        sd_mock.query_devices.side_effect = OSError("PortAudio error")
        with patch.dict(sys.modules, {"sounddevice": sd_mock}):
            with caplog.at_level(logging.DEBUG, logger="ai_voice_interconnector.audio"):
                devices, degraded = get_audio_devices_with_status()
        assert degraded is True
        assert devices == [{"id": 0, "name": "Default", "latency": 0.1}]
        assert any(
            "enumeración" in r.message.lower() and r.exc_info for r in caplog.records
        ), "el fallo de enumeración debe registrar un debug con traza"


class _FakeCaptureDevice:
    """Doble de `miniaudio.CaptureDevice`: al recibir `start(gen)`, empuja
    `blocks` (bytes int16 crudos) al generador vía `.send(...)`, simulando el
    callback nativo de miniaudio, y registra el ciclo de vida (prime/stop)."""

    def __init__(self, blocks):
        self._blocks = blocks
        self.stop_calls = 0
        self.gen = None

    def start(self, gen):
        self.gen = gen
        for block in self._blocks:
            gen.send(block)

    def stop(self):
        self.stop_calls += 1


class TestAudioRecorder:
    def test_record_fixed_returns_normalized_mono_float32(self):
        """Bloques int16 conocidos, concatenados y normalizados por
        INT16_MAX_F, en un ndarray float32 mono."""
        samples = np.array([0, 16384, -16384, 32767], dtype=np.int16)
        blocks = [samples[:2].tobytes(), samples[2:].tobytes()]
        device = _FakeCaptureDevice(blocks)

        result = AudioRecorder(capture_factory=lambda: device).record_fixed(seconds=0.01)

        assert result.dtype == np.float32
        assert result.ndim == 1
        np.testing.assert_allclose(result, samples.astype(np.float32) / INT16_MAX_F, atol=1e-6)

    def test_lifecycle_primes_generator_before_start_and_stops_once(self):
        """`next(gen)` prima el generador antes de `device.start(gen)`, y
        `device.stop()` se invoca exactamente una vez al terminar."""
        import inspect

        primed_before_start = []

        class _TrackingDevice(_FakeCaptureDevice):
            def start(self, gen):
                # El generador ya debe estar primado (suspendido en su
                # primer `yield`) antes de que el device reciba el control.
                primed_before_start.append(inspect.getgeneratorstate(gen) == "GEN_SUSPENDED")
                super().start(gen)

        device = _TrackingDevice([np.array([1000], dtype=np.int16).tobytes()])

        AudioRecorder(capture_factory=lambda: device).record_fixed(seconds=0.01)

        assert primed_before_start == [True]
        assert device.stop_calls == 1

    def test_record_until_enter_stops_on_input_line(self):
        """`record_until_enter()` graba hasta que `input()` retorna (Enter),
        sin depender de una tecla real."""
        blocks = [np.array([0, 32767], dtype=np.int16).tobytes()]
        device = _FakeCaptureDevice(blocks)

        with patch("builtins.input", return_value=""):
            result = AudioRecorder(capture_factory=lambda: device).record_until_enter()

        assert device.stop_calls == 1
        np.testing.assert_allclose(
            result, np.array([0, 32767], dtype=np.float32) / INT16_MAX_F, atol=1e-6
        )


class TestEncodePcmInt16B64:
    """`encode_pcm_int16_b64`: float32 mono → PCM int16 crudo en base64 ASCII,
    el canal cliente→daemon de /transcribe (la captura es de cliente)."""

    def test_roundtrip_float32_to_int16(self):
        samples = np.array([0.0, 0.5, -0.5, 1.0, -1.0, 0.25], dtype=np.float32)
        encoded = encode_pcm_int16_b64(samples)

        decoded = np.frombuffer(base64.b64decode(encoded), dtype=np.int16)
        np.testing.assert_array_equal(decoded, (samples * INT16_MAX_F).astype(np.int16))

    def test_returns_ascii_base64(self):
        encoded = encode_pcm_int16_b64(np.array([0.1, -0.2], dtype=np.float32))
        assert isinstance(encoded, str)
        base64.b64decode(encoded)

    def test_size_scales_by_base64_factor(self):
        """N muestras int16 (2 bytes) → 2N bytes → ceil(2N/3)*4 chars base64."""
        n = 1000
        encoded = encode_pcm_int16_b64(np.zeros(n, dtype=np.float32))
        assert len(encoded) == ((2 * n + 2) // 3) * 4


class TestGetCaptureDevices:
    def test_get_captures_translates_to_common_shape(self):
        from ai_voice_interconnector.audio import get_capture_devices_with_status

        fake_miniaudio = MagicMock()
        fake_miniaudio.Devices.return_value.get_captures.return_value = [
            {"id": 3, "name": "Micrófono USB"},
        ]
        with patch.dict(sys.modules, {"miniaudio": fake_miniaudio}):
            devices, degraded = get_capture_devices_with_status()

        assert degraded is False
        assert devices == [{"id": 3, "name": "Micrófono USB", "latency": 0.1}]

    def test_get_captures_failure_degrades_to_fallback(self, caplog):
        import logging

        from ai_voice_interconnector.audio import get_capture_devices_with_status

        fake_miniaudio = MagicMock()
        fake_miniaudio.Devices.return_value.get_captures.side_effect = OSError("sin backend")
        with patch.dict(sys.modules, {"miniaudio": fake_miniaudio}):
            with caplog.at_level(logging.DEBUG, logger="ai_voice_interconnector.audio"):
                devices, degraded = get_capture_devices_with_status()

        assert degraded is True
        assert devices == [{"id": 0, "name": "Default", "latency": 0.1}]
        assert any(
            "enumeración" in r.message.lower() and r.exc_info for r in caplog.records
        ), "el fallo de enumeración de captura debe registrar un debug con traza"
