"""
Escritura de audio a WAV (en memoria o a archivo).

Extraído de `ChatterboxEngine`: fusiona `_audio_to_wav` (conversión de
un array/tensor a bytes WAV) y `_save_wav` (escritura a disco).
"""

import io
import wave
from pathlib import Path

import numpy as np


class AudioWriter:
    """Convierte audio a bytes WAV PCM 16-bit mono."""

    def write(self, audio_data, sample_rate: int) -> bytes:
        """Convierte `audio_data` a bytes WAV PCM 16-bit mono y los retorna.

        El sample rate se pasa explícitamente (el orquestador lo toma de
        `tts.sr`) en vez de leerse de `self._tts`.
        """
        if hasattr(audio_data, 'numpy'):
            audio_np = audio_data.numpy()
        elif hasattr(audio_data, 'cpu'):
            audio_np = audio_data.cpu().numpy()
        else:
            audio_np = np.array(audio_data)

        # Asegura que sea float32 en [-1, 1]
        if audio_np.dtype != np.float32:
            audio_np = audio_np.astype(np.float32)

        # Maneja la dimensión de batch
        if audio_np.ndim > 1:
            audio_np = audio_np.flatten()

        # Normaliza si hace falta
        max_val = np.abs(audio_np).max()
        if max_val > 1.0:
            audio_np = audio_np / max_val

        wav_bytes = self._to_wav_bytes(audio_np, sample_rate)

        return wav_bytes

    @staticmethod
    def _to_wav_bytes(audio_np: np.ndarray, sample_rate: int) -> bytes:
        """Codifica el array float32 aplanado a bytes WAV en un buffer en memoria."""
        # Fade adaptativo: detecta dinámicamente dónde termina el habla real
        # (RMS > 0.004) y aplica fade-out desde ese punto para eliminar el hiss
        # de alta frecuencia (4-8 kHz) que el vocoder S3Gen produce en la cola.
        # El silencio de cola (50 ms) da margen al DAC para drenar sin chasquido.
        threshold_rms = 0.004
        window_ms = 10
        window_samples = int(sample_rate * window_ms / 1000)
        min_fade_ms = 15
        fade_start = 0

        i = len(audio_np) - window_samples
        while i >= 0:
            segment = audio_np[i:i + window_samples]
            rms = float(np.sqrt(np.mean(segment ** 2)))
            if rms > threshold_rms:
                fade_start = i + window_samples
                break
            i -= window_samples

        fade_samples = max(int(sample_rate * min_fade_ms / 1000), len(audio_np) - fade_start)
        fade_samples = min(fade_samples, len(audio_np))

        if fade_samples > 0 and len(audio_np) >= fade_samples:
            fade_window = np.linspace(1.0, 0.0, fade_samples, dtype=np.float32)
            audio_np = audio_np.copy()
            audio_np[-fade_samples:] *= fade_window

        tail_silence = np.zeros(int(sample_rate * 0.05), dtype=np.float32)
        audio_np = np.concatenate([audio_np, tail_silence])

        buffer = io.BytesIO()
        with wave.open(buffer, 'wb') as wf:
            wf.setnchannels(1)
            wf.setsampwidth(2)  # 16 bits
            wf.setframerate(sample_rate)
            audio_int16 = (audio_np * 32767).astype(np.int16)
            wf.writeframes(audio_int16.tobytes())
        return buffer.getvalue()
