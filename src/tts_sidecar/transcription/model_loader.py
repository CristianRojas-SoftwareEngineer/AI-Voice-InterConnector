"""
Carga y resolución del modelo de transcripción `faster-whisper`.

Colaborador inyectable que carga y cachea en memoria un modelo de
transcripción sobre el runtime CT2 ya embarcado por la traducción (espeja el
patrón `TranslationModelLoader` de `tts_sidecar.translation.model_loader`).
"""

from pathlib import Path

from ..exceptions import TranscriptionModelMissingError


def _default_model_factory(path: str):
    """Factory de producción: `faster_whisper.WhisperModel` sobre el runtime
    CT2 embarcado.

    Import diferido (dentro de la función) para no arrastrar el nativo en
    comandos que nunca transcriben, y para que el módulo siga siendo
    importable en entornos donde faster-whisper aún no esté instalado (solo
    falla al transcribir de verdad).
    """
    import faster_whisper

    return faster_whisper.WhisperModel(path, device="cpu", compute_type="int8")


class WhisperModelLoader:
    """Resuelve y carga en memoria el modelo de transcripción CT2 desde una
    ruta de caché.

    El constructor del modelo (`model_factory`) es inyectable solo por
    testabilidad: en producción usa `faster_whisper.WhisperModel` (runtime
    embarcado); en tests, un doble simple que no requiere el runtime nativo.
    """

    def __init__(self, model_factory=None):
        self._model_factory = model_factory or _default_model_factory
        self._cache: dict[str, object] = {}

    def load(self, cache_dir):
        """Carga el modelo desde `cache_dir`, reutilizando la instancia
        cacheada en llamadas repetidas con la misma ruta."""
        cache_dir = Path(cache_dir)
        key = str(cache_dir)

        if key in self._cache:
            return self._cache[key]

        if not cache_dir.exists():
            raise TranscriptionModelMissingError(
                f"Modelo de transcripción no encontrado en {cache_dir}. "
                "Ejecuta 'tts-sidecar setup --with-stt' para provisionarlo."
            )

        model = self._model_factory(key)
        self._cache[key] = model
        return model

    def is_loaded(self, cache_dir) -> bool:
        """Indica si `cache_dir` ya está caliente en memoria (sin cargarlo)."""
        return str(Path(cache_dir)) in self._cache
