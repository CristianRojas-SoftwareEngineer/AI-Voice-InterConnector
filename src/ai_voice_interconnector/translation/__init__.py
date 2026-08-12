"""
Subpaquete de traducción de texto local `es<->en`.

Pipeline validación -> segmentación -> traducción -> ensamblado que se
inserta antes de la síntesis para que el usuario escriba en su idioma nativo
y obtenga audio en el idioma destino con su propia voz clonada.
"""

from .model_loader import TranslationModelLoader, resolve_language, model_pair_name
from .segmenter import SentenceSegmenter
from .translator import MarianTranslator
from .assembler import SegmentAssembler
from .service import TranslationService, default_cache_dir

__all__ = [
    "TranslationModelLoader",
    "resolve_language",
    "model_pair_name",
    "SentenceSegmenter",
    "MarianTranslator",
    "SegmentAssembler",
    "TranslationService",
    "default_cache_dir",
]
