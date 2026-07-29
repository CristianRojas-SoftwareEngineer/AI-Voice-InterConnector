"""Tests para AudioWriter: conversión a WAV.

El módulo es puro (no carga el modelo); los tests usan numpy y re-leen el WAV
resultante con el módulo `wave` para validar la codificación PCM 16-bit mono.
"""

import sys
import wave
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from tts_sidecar.audio_writer import AudioWriter


def _read_wav(wav_bytes: bytes):
    """Decodifica bytes WAV y retorna (sample_rate, n_channels, n_frames, int16)."""
    import io
    with wave.open(io.BytesIO(wav_bytes), 'rb') as wf:
        sr = wf.getframerate()
        ch = wf.getnchannels()
        n = wf.getnframes()
        raw = wf.readframes(n)
    int16 = np.frombuffer(raw, dtype=np.int16)
    return sr, ch, n, int16


class TestWriteInMemory:
    def test_returns_valid_mono_16bit_wav(self):
        audio = np.zeros(24000, dtype=np.float32)
        audio[0] = 1.0
        audio[-1] = -1.0
        out = AudioWriter().write(audio, sample_rate=24000)
        assert isinstance(out, bytes)
        sr, ch, n, int16 = _read_wav(out)
        assert sr == 24000
        assert ch == 1
        tail_frames = int(24000 * 0.05)  # 50 ms de silencio de cola
        assert n == 24000 + tail_frames
        assert int16[0] == 32767
        assert int16[-1] == 0  # Silencio al final del tail

    def test_float64_input_is_cast_to_float32(self):
        audio = np.linspace(-0.5, 0.5, 1000)  # float64 por defecto
        out = AudioWriter().write(audio, sample_rate=24000)
        sr, ch, n, _ = _read_wav(out)
        tail_frames = int(24000 * 0.05)
        assert n == 1000 + tail_frames
        assert ch == 1

    def test_batched_input_is_flattened(self):
        # (1, N) batch -> se aplana a N
        audio = np.full((1, 500), 0.25, dtype=np.float32)
        out = AudioWriter().write(audio, sample_rate=16000)
        _, _, n, _ = _read_wav(out)
        tail_frames = int(16000 * 0.05)
        assert n == 500 + tail_frames

    def test_clips_values_above_one(self):
        audio = np.full(100, 2.0, dtype=np.float32)  # fuera de [-1, 1]
        out = AudioWriter().write(audio, sample_rate=24000)
        _, _, _, int16 = _read_wav(out)
        assert int16.max() == 32767  # normalizado, sin desbordamiento

    def test_write_does_not_touch_disk(self, tmp_path):
        audio = np.full(100, 0.1, dtype=np.float32)
        AudioWriter().write(audio, sample_rate=24000)
        assert not any(tmp_path.iterdir())
