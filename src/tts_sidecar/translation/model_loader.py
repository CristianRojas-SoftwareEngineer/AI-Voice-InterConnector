"""
Carga y resolución del modelo de traducción `opus-mt`.

Colaborador inyectable que carga y cachea en memoria un modelo de traducción
convertido a formato CT2 desde una ruta de caché dada (espeja el patrón
`ModelLoader` de `tts_sidecar.model_loader`, usado por el motor de síntesis).

Además define la normalización de la taxonomía de idiomas del proyecto
(`es-latam` de síntesis vs. `es` ISO del traductor) y la derivación del
identificador de par de modelo `opus-mt-{origen}-{destino}`.
"""

from pathlib import Path

from ..exceptions import TranslationModelMissingError, UnsupportedLanguagePairError

# Idiomas soportados por el subsistema de traducción (decisión cerrada del plan).
_SUPPORTED_LANGUAGES = {"es", "en"}

# Alias de la taxonomía de síntesis que requieren normalización al ISO 639-1
# que usa el traductor. Cualquier idioma no listado se devuelve intacto (ya
# es ISO, p. ej. "en").
_LANGUAGE_ALIASES = {"es-latam": "es"}


def resolve_language(language: str) -> str:
    """Normaliza un idioma de la taxonomía de síntesis al ISO usado por el traductor.

    `es-latam` -> `es`; cualquier otro valor (p. ej. `en`, `es`) se devuelve
    intacto.
    """
    return _LANGUAGE_ALIASES.get(language, language)


def model_pair_name(source: str, target: str) -> str:
    """Deriva el identificador de par `opus-mt-{origen}-{destino}` a partir de
    idiomas ya normalizados. Eleva `UnsupportedLanguagePairError` si alguno de
    los dos no está entre los idiomas soportados (`es`/`en`)."""
    if source not in _SUPPORTED_LANGUAGES or target not in _SUPPORTED_LANGUAGES:
        raise UnsupportedLanguagePairError(
            f"Par de idiomas no soportado: {source}->{target} "
            f"(soportados: {sorted(_SUPPORTED_LANGUAGES)})"
        )
    return f"opus-mt-{source}-{target}"


class _MarianCT2Model:
    """Envuelve `ctranslate2.Translator` + los SentencePiece origen/destino de
    un modelo `opus-mt` convertido, exponiendo `.translate(str) -> str`: el
    contrato de texto->texto que `MarianTranslator._default_backend` asume.

    `ctranslate2.Translator` por sí solo NO expone `.translate(str)`: opera
    sobre listas de subtokens (requiere tokenizar/detokenizar con el
    SentencePiece del par, provisionado junto al modelo por
    `ct2-transformers-converter --copy_files source.spm target.spm`, ver
    `cmd_setup`). Esta clase cierra ese contrato en un solo punto.
    """

    def __init__(self, path: str):
        import ctranslate2
        import sentencepiece

        model_dir = Path(path)
        self._translator = ctranslate2.Translator(str(model_dir))
        self._sp_source = sentencepiece.SentencePieceProcessor(
            model_file=str(model_dir / "source.spm")
        )
        self._sp_target = sentencepiece.SentencePieceProcessor(
            model_file=str(model_dir / "target.spm")
        )

    def translate(self, text: str) -> str:
        # Los modelos Marian/opus-mt esperan el origen terminado en `</s>`
        # (así lo antepone `MarianTokenizer` en HuggingFace) y el conversor
        # CT2 (`ct2-transformers-converter`) NO lo añade automáticamente
        # (`add_source_eos: false` en el `config.json` resultante). Sin este
        # token el encoder nunca recibe una marca de fin de secuencia y el
        # decoder jamás muestrea `</s>`: entra en un loop de repetición hasta
        # el tope de longitud de `translate_batch`.
        tokens = self._sp_source.encode(text, out_type=str)
        tokens.append("</s>")
        results = self._translator.translate_batch([tokens])
        output_tokens = results[0].hypotheses[0]
        return self._sp_target.decode(output_tokens)


def _default_translator_factory(path: str):
    """Factory de producción: modelo CT2 embarcado + tokenización SentencePiece.

    Import diferido (dentro de `_MarianCT2Model.__init__`) para no arrastrar
    las librerías nativas en comandos que nunca traducen, y para que el
    módulo siga siendo importable en entornos donde ctranslate2/sentencepiece
    aún no estén instalados (solo falla al traducir de verdad).
    """
    return _MarianCT2Model(path)


class TranslationModelLoader:
    """Resuelve y carga en memoria el modelo de traducción CT2 desde una ruta
    de caché.

    El constructor del modelo (`translator_factory`) es inyectable solo por
    testabilidad: en producción usa `ctranslate2.Translator` (runtime
    embarcado); en tests, un doble simple que no requiere el runtime nativo.
    """

    def __init__(self, translator_factory=None):
        self._translator_factory = translator_factory or _default_translator_factory
        self._cache: dict[str, object] = {}

    def load(self, cache_dir):
        """Carga el modelo desde `cache_dir`, reutilizando la instancia
        cacheada en llamadas repetidas con la misma ruta."""
        cache_dir = Path(cache_dir)
        key = str(cache_dir)

        if key in self._cache:
            return self._cache[key]

        if not cache_dir.exists():
            raise TranslationModelMissingError(
                f"Modelo de traducción no encontrado en {cache_dir}. "
                "Ejecuta 'tts-sidecar setup --language en' para provisionarlo."
            )

        model = self._translator_factory(key)
        self._cache[key] = model
        return model

    def is_loaded(self, cache_dir) -> bool:
        """Indica si `cache_dir` ya está caliente en memoria (sin cargarlo)."""
        return str(Path(cache_dir)) in self._cache
