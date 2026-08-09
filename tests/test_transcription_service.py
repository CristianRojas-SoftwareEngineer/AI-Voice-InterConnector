"""
Tests deterministas de `TranscriptionService`: orquesta el pipeline completo
(resolver idioma -> fail-fast del modelo -> leer audio -> transcribir) con
los colaboradores inyectados, y una regresión sobre un WAV real generado en
el propio test contra un `transcriber` doblado (nunca faster-whisper real).
"""

import wave

import numpy as np

from tts_sidecar.transcription.service import TranscriptionService, _default_audio_reader


class _FakeLoader:
    """Doble de `WhisperModelLoader`: siempre "carga" con éxito y registra
    orden de invocación en una lista compartida. Cachea por `cache_dir`
    (espeja `WhisperModelLoader.load`, `model_loader.py:47-48`): la segunda
    invocación con el mismo `cache_dir` (p. ej. la que hace
    `transcribe_samples` al ser delegada desde `transcribe`) es barata y no
    vuelve a registrar orden."""

    def __init__(self, order_log):
        self._order_log = order_log
        self._loaded_dirs = set()

    def load(self, cache_dir):
        if cache_dir not in self._loaded_dirs:
            self._order_log.append("load")
            self._loaded_dirs.add(cache_dir)
        return "fake-model"


def test_transcribe_loads_model_before_reading_audio_fail_fast():
    """El modelo se carga (fail-fast) ANTES de invocar el audio_reader."""
    order_log = []
    loader = _FakeLoader(order_log)

    def fake_audio_reader(path):
        order_log.append("read_audio")
        return "fake-audio"

    class _FakeTranscriber:
        def transcribe(self, audio, language, cache_dir):
            return "texto"

    service = TranscriptionService(
        model_loader=loader,
        transcriber=_FakeTranscriber(),
        audio_reader=fake_audio_reader,
        cache_dir_resolver=lambda: "/ruta/cache",
    )

    service.transcribe("nota.wav", "es-latam")

    assert order_log == ["load", "read_audio"]


def test_transcribe_samples_resolves_language_and_delegates():
    """`transcribe_samples` hace fail-fast y delega en el transcriber, sin
    tocar el `audio_reader` (las muestras ya vienen decodificadas)."""
    order_log = []
    loader = _FakeLoader(order_log)

    def fake_audio_reader(path):
        order_log.append("read_audio")
        return "no-debe-invocarse"

    class _FakeTranscriber:
        def transcribe(self, audio, language, cache_dir):
            order_log.append("transcribe")
            return "texto"

    service = TranscriptionService(
        model_loader=loader,
        transcriber=_FakeTranscriber(),
        audio_reader=fake_audio_reader,
        cache_dir_resolver=lambda: "/ruta/cache",
    )

    result = service.transcribe_samples("muestras", "es-latam")

    assert result == "texto"
    assert order_log == ["load", "transcribe"]


class _RecordingTranscriber:
    """Doble de `WhisperTranscriber`: devuelve texto fijo y registra el
    idioma con el que se le llamó."""

    def __init__(self):
        self.calls = []

    def transcribe(self, audio, language, cache_dir):
        self.calls.append((audio, language, cache_dir))
        return "texto transcrito"


def test_transcribe_runs_full_pipeline_with_language_resolved():
    """`source_language="es-latam"` llega a `transcriber.transcribe` ya
    resuelto a `"es"`; el texto devuelto por el transcriber se propaga tal cual."""
    loader = _FakeLoader([])
    transcriber = _RecordingTranscriber()

    service = TranscriptionService(
        model_loader=loader,
        transcriber=transcriber,
        audio_reader=lambda path: "fake-audio",
        cache_dir_resolver=lambda: "/ruta/cache",
    )

    result = service.transcribe("nota.wav", "es-latam")

    assert result == "texto transcrito"
    assert transcriber.calls == [("fake-audio", "es", "/ruta/cache")]


def _write_wav(path, n_channels: int, samples: np.ndarray, framerate: int = 16000) -> None:
    """Escribe un WAV int16 simple (stdlib `wave`) con `n_channels` canales
    a la frecuencia `framerate` (16 kHz por defecto)."""
    with wave.open(str(path), "wb") as wav_file:
        wav_file.setnchannels(n_channels)
        wav_file.setsampwidth(2)
        wav_file.setframerate(framerate)
        wav_file.writeframes(samples.astype(np.int16).tobytes())


def test_default_audio_reader_real_wav_mono_normalized(tmp_path):
    """Ejercita el lector `wave` real (sin PyAV) contra un WAV mono generado
    en el test: normaliza int16 -> float32 con INT16_MAX_F."""
    wav_path = tmp_path / "tono.wav"
    samples = np.array([0, 16384, -16384, 32767], dtype=np.int16)
    _write_wav(wav_path, n_channels=1, samples=samples)

    result = _default_audio_reader(wav_path)

    assert result.dtype == np.float32
    np.testing.assert_allclose(result, samples.astype(np.float32) / 32768.0, atol=1e-6)


def test_default_audio_reader_real_wav_stereo_downmixed_to_mono(tmp_path):
    """Un WAV estéreo se downmixea a mono (promedio de canales)."""
    wav_path = tmp_path / "tono_stereo.wav"
    # Frames intercalados L, R, L, R...
    interleaved = np.array([0, 32767, 16384, -16384], dtype=np.int16)
    _write_wav(wav_path, n_channels=2, samples=interleaved)

    result = _default_audio_reader(wav_path)

    assert result.shape == (2,)
    expected = interleaved.astype(np.float32).reshape(-1, 2).mean(axis=1) / 32768.0
    np.testing.assert_allclose(result, expected, atol=1e-6)


def test_default_audio_reader_resamples_non_16k_to_16k(tmp_path):
    """Un WAV a frecuencia distinta de 16 kHz se remuestrea a 16 kHz (Whisper
    asume 16 kHz para un `np.ndarray`): un mono a 32 kHz de N muestras devuelve
    ~N/2 muestras interpoladas linealmente."""
    wav_path = tmp_path / "tono_32k.wav"
    samples = np.array([0, 8192, 16384, 24576, 32767, -16384], dtype=np.int16)
    _write_wav(wav_path, n_channels=1, samples=samples, framerate=32000)

    result = _default_audio_reader(wav_path)

    assert result.dtype == np.float32
    assert result.shape == (3,)  # 6 muestras a 32 kHz -> 3 a 16 kHz
    source = samples.astype(np.float32) / 32768.0
    expected = np.interp(
        np.linspace(0, len(source), 3, endpoint=False),
        np.arange(len(source)),
        source,
    ).astype(np.float32)
    np.testing.assert_allclose(result, expected, atol=1e-6)
