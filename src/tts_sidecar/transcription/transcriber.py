"""
Transcripción de audio con el modelo `faster-whisper` resuelto por
`WhisperModelLoader`.

El backend de runtime es un colaborador inyectable **solo por testabilidad**:
en producción invoca el modelo CT2 embarcado (decisión cerrada, sin segundo
runtime); en tests, un doble simple sin dependencia del runtime nativo. Un
fallo del backend se propaga siempre como `TranscriptionFailedError`, con
identidad propia distinta de `TranscriptionModelMissingError` (esa la eleva
el loader cuando el modelo ni siquiera está provisionado).
"""

from ..exceptions import TranscriptionFailedError


def _default_backend(model, audio, language) -> str:
    """Backend de producción: delega en la interfaz de transcripción del
    modelo cargado por `WhisperModelLoader`. Whisper SOLO transcribe
    (`task="transcribe"`), nunca traduce."""
    segments, _info = model.transcribe(audio, language=language, task="transcribe")
    return "".join(segment.text for segment in segments)


class WhisperTranscriber:
    """Transcribe audio con el modelo `faster-whisper` resuelto vía
    `WhisperModelLoader` inyectado."""

    def __init__(self, model_loader, backend=None):
        self._model_loader = model_loader
        self._backend = backend or _default_backend

    def transcribe(self, audio, language: str, cache_dir) -> str:
        """Transcribe `audio` en `language` usando el modelo cacheado en
        `cache_dir`. Cualquier fallo del backend se eleva como
        `TranscriptionFailedError`."""
        model = self._model_loader.load(cache_dir)
        try:
            return self._backend(model, audio, language)
        except Exception as exc:
            raise TranscriptionFailedError(
                f"Fallo al transcribir audio (idioma={language}): {exc}"
            ) from exc
