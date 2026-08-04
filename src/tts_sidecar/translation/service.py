"""
Orquestación del pipeline completo de traducción: valida -> segmenta ->
traduce -> ensambla, con atajo de passthrough cuando origen y destino
coinciden (sin cargar ningún modelo).

Los cuatro colaboradores (loader, segmenter, translator, assembler) son
inyectables: en producción, las implementaciones reales del subpaquete
(`TranslationModelLoader`, `SentenceSegmenter`, `MarianTranslator`,
`SegmentAssembler`); en tests, dobles simples que nunca tocan el runtime CT2.
"""

from pathlib import Path

from .model_loader import resolve_language, model_pair_name
from ..paths import data_root


def default_cache_dir(source: str, target: str) -> str:
    """Ruta de caché por defecto del modelo `opus-mt-{source}-{target}`,
    dentro de la raíz de datos de usuario del proyecto (misma convención que
    el resto de la caché de modelos)."""
    return str(Path(data_root()) / "translation-models" / model_pair_name(source, target))


class TranslationService:
    """Orquesta el pipeline de traducción `es<->en` de punta a punta."""

    def __init__(self, model_loader, segmenter, translator, assembler, cache_dir_resolver=None):
        self._model_loader = model_loader
        self._segmenter = segmenter
        self._translator = translator
        self._assembler = assembler
        self._cache_dir_resolver = cache_dir_resolver or default_cache_dir

    def translate(self, texto: str, origen: str, destino: str) -> str:
        """Traduce `texto` de `origen` a `destino`. Si `origen == destino`,
        devuelve `texto` intacto sin cargar ningún modelo (passthrough)."""
        if origen == destino:
            return texto

        source = resolve_language(origen)
        target = resolve_language(destino)
        cache_dir = self._cache_dir_resolver(source, target)

        # Fail-fast: valida que el modelo esté disponible antes de segmentar,
        # para no hacer trabajo de segmentación sobre un modelo ausente.
        self._model_loader.load(cache_dir)

        paragraphs = self._segmenter.segment(texto, source)
        translated = [
            [self._translator.translate(segment, source, target, cache_dir) for segment in segments]
            for segments in paragraphs
        ]
        return self._assembler.assemble(translated)
