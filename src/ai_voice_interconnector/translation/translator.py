"""
Traducción por segmento con el modelo `opus-mt` resuelto por
`TranslationModelLoader`.

El backend de runtime es un colaborador inyectable **solo por testabilidad**:
en producción invoca el modelo CT2 embarcado (decisión cerrada, sin segundo
runtime); en tests, un doble simple sin dependencia del runtime nativo. Un
fallo del backend se propaga siempre como `TranslationFailedError`, con
identidad propia distinta de `TranslationModelMissingError` (esa la eleva el
loader cuando el modelo ni siquiera está provisionado).
"""

from ..exceptions import TranslationFailedError


def _default_backend(model, segment: str) -> str:
    """Backend de producción: delega en la interfaz de traducción de texto
    del modelo cargado por `TranslationModelLoader`."""
    return model.translate(segment)


class MarianTranslator:
    """Traduce segmentos de texto usando el modelo `opus-mt` del par
    (origen, destino), resuelto vía `TranslationModelLoader` inyectado."""

    def __init__(self, model_loader, backend=None):
        self._model_loader = model_loader
        self._backend = backend or _default_backend

    def translate(self, segment: str, source: str, target: str, cache_dir) -> str:
        """Traduce `segment` de `source` a `target` usando el modelo cacheado
        en `cache_dir`. Cualquier fallo del backend se eleva como
        `TranslationFailedError`."""
        model = self._model_loader.load(cache_dir)
        try:
            return self._backend(model, segment)
        except Exception as exc:
            raise TranslationFailedError(
                f"Fallo al traducir segmento ({source}->{target}): {exc}"
            ) from exc
