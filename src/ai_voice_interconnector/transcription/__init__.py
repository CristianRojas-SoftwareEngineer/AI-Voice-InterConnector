"""
Subpaquete de transcripción de archivo WAV a texto (`faster-whisper` sobre
el runtime CT2 ya embarcado).

Whisper SOLO transcribe (`task="transcribe"`), nunca traduce ni sintetiza:
ese rol permanece en `translation/` y en el motor de síntesis.
"""

from .model_loader import WhisperModelLoader
from .transcriber import WhisperTranscriber
from .service import TranscriptionService, default_cache_dir

__all__ = [
    "WhisperModelLoader",
    "WhisperTranscriber",
    "TranscriptionService",
    "default_cache_dir",
]
