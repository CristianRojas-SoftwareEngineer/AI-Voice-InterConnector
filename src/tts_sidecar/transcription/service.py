"""
Orquestación del pipeline de transcripción: resuelve idioma -> fail-fast del
modelo -> lee WAV -> transcribe.

Los cuatro colaboradores (model_loader, transcriber, audio_reader,
cache_dir_resolver) son inyectables: en producción, `WhisperModelLoader` +
`WhisperTranscriber` + el lector `wave` real de la stdlib; en tests, dobles
simples que nunca tocan el runtime CT2 ni PyAV.
"""

import wave
from pathlib import Path

import numpy as np

from ..translation.model_loader import resolve_language
from ..audio import INT16_MAX_F
from ..paths import data_root


# Frecuencia que `faster_whisper.WhisperModel.transcribe` asume cuando recibe
# un `np.ndarray`: pasar audio a otra frecuencia degrada la transcripción.
WHISPER_SAMPLE_RATE = 16000


def _resample_to_whisper_rate(samples: np.ndarray, source_rate: int) -> np.ndarray:
    """Remuestrea `samples` (mono, float32) de `source_rate` a
    `WHISPER_SAMPLE_RATE` por interpolación lineal. Si ya está a 16 kHz,
    devuelve el array intacto (sin dependencias externas)."""
    if source_rate == WHISPER_SAMPLE_RATE:
        return samples
    n_out = round(len(samples) * WHISPER_SAMPLE_RATE / source_rate)
    resampled = np.interp(
        np.linspace(0, len(samples), n_out, endpoint=False),
        np.arange(len(samples)),
        samples,
    )
    return resampled.astype(np.float32)


def default_cache_dir() -> str:
    """Ruta de caché por defecto del modelo `faster-whisper-small`, dentro
    de la raíz de datos de usuario del proyecto (misma convención que la
    caché de modelos de traducción)."""
    return str(Path(data_root()) / "transcription-models" / "faster-whisper-small")


def _default_audio_reader(path) -> np.ndarray:
    """Lector de audio de producción: decodifica un WAV con la stdlib
    (`wave`, sin PyAV), downmixea a mono si es estéreo (promedio de canales),
    normaliza int16 -> float32 con `INT16_MAX_F` (misma constante que usa la
    reproducción de audio, sin duplicarla) y remuestrea a los 16 kHz que
    Whisper asume para un `np.ndarray`."""
    with wave.open(str(path), "rb") as wav_file:
        n_channels = wav_file.getnchannels()
        source_rate = wav_file.getframerate()
        raw = wav_file.readframes(wav_file.getnframes())

    samples = np.frombuffer(raw, dtype=np.int16).astype(np.float32) / INT16_MAX_F
    if n_channels > 1:
        samples = samples.reshape(-1, n_channels).mean(axis=1)
    return _resample_to_whisper_rate(samples, source_rate)


class TranscriptionService:
    """Orquesta el pipeline de transcripción de archivo WAV a texto de punta
    a punta."""

    def __init__(self, model_loader, transcriber, audio_reader=None, cache_dir_resolver=None):
        self._model_loader = model_loader
        self._transcriber = transcriber
        self._audio_reader = audio_reader or _default_audio_reader
        self._cache_dir_resolver = cache_dir_resolver or default_cache_dir

    def transcribe_samples(self, samples, source_language: str) -> str:
        """Transcribe `samples` (`np.ndarray` ya decodificado, p. ej. desde
        captura de micrófono), hablado en `source_language` (taxonomía CLI,
        normalizada internamente vía `resolve_language`)."""
        language = resolve_language(source_language)
        cache_dir = self._cache_dir_resolver()

        # Fail-fast: valida que el modelo esté disponible antes de transcribir.
        self._model_loader.load(cache_dir)

        return self._transcriber.transcribe(samples, language, cache_dir)

    def transcribe(self, audio_path, source_language: str) -> str:
        """Transcribe el WAV en `audio_path`, hablado en `source_language`
        (taxonomía CLI, normalizada internamente vía `resolve_language`)."""
        language = resolve_language(source_language)
        cache_dir = self._cache_dir_resolver()

        # Fail-fast: valida que el modelo esté disponible ANTES de decodificar
        # el WAV (self._audio_reader), para no leer el archivo sobre un
        # modelo ausente. Debe evaluarse en este orden explícito: una
        # envoltura `transcribe_samples(self._audio_reader(audio_path), ...)`
        # invertiría el orden (Python evalúa argumentos antes de invocar).
        self._model_loader.load(cache_dir)

        audio = self._audio_reader(audio_path)
        return self.transcribe_samples(audio, source_language)
