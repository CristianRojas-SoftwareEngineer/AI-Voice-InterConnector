"""
Tests deterministas de `TranslationModelLoader` y las funciones de resolución
de idioma/par del subpaquete de traducción.

El constructor de traductor CT2 es inyectable (`translator_factory`) solo por
testabilidad: estos tests nunca instancian `ctranslate2.Translator` real, solo
verifican la carga/caché en memoria y la resolución de idioma/par.
"""

import pytest

from ai_voice_interconnector.exceptions import TranslationModelMissingError, UnsupportedLanguagePairError
from ai_voice_interconnector.translation.model_loader import (
    TranslationModelLoader,
    resolve_language,
    model_pair_name,
)


def test_load_returns_model_from_injected_factory(tmp_path):
    """Carga exitosa desde una ruta de caché simulada, vía el factory inyectado."""
    cache_dir = tmp_path / "opus-mt-es-en"
    cache_dir.mkdir()
    calls = []

    def fake_factory(path):
        calls.append(path)
        return "fake-model"

    loader = TranslationModelLoader(translator_factory=fake_factory)
    model = loader.load(cache_dir)

    assert model == "fake-model"
    assert calls == [str(cache_dir)]


def test_load_reuses_cached_instance(tmp_path):
    """Llamadas repetidas con la misma ruta no vuelven a invocar el factory."""
    cache_dir = tmp_path / "opus-mt-es-en"
    cache_dir.mkdir()
    calls = []

    def fake_factory(path):
        calls.append(path)
        return object()

    loader = TranslationModelLoader(translator_factory=fake_factory)
    first = loader.load(cache_dir)
    second = loader.load(cache_dir)

    assert first is second
    assert len(calls) == 1


def test_is_loaded_false_before_load_true_after(tmp_path):
    """`is_loaded` refleja el estado de la caché en memoria sin cargar nada."""
    cache_dir = tmp_path / "opus-mt-es-en"
    cache_dir.mkdir()
    loader = TranslationModelLoader(translator_factory=lambda path: "fake-model")

    assert loader.is_loaded(cache_dir) is False
    loader.load(cache_dir)
    assert loader.is_loaded(cache_dir) is True


def test_load_missing_cache_dir_raises():
    """Una ruta de caché inexistente eleva TranslationModelMissingError."""
    loader = TranslationModelLoader(translator_factory=lambda path: "unused")

    with pytest.raises(TranslationModelMissingError):
        loader.load("/no/existe/opus-mt-es-en")


@pytest.mark.parametrize(
    "language,expected",
    [
        ("es-latam", "es"),
        ("en", "en"),
        ("es", "es"),
    ],
)
def test_resolve_language(language, expected):
    assert resolve_language(language) == expected


@pytest.mark.parametrize(
    "source,target,expected",
    [
        ("es", "en", "opus-mt-es-en"),
        ("en", "es", "opus-mt-en-es"),
        ("es", "es", "opus-mt-es-es"),
        ("en", "en", "opus-mt-en-en"),
    ],
)
def test_model_pair_name_four_pairs(source, target, expected):
    assert model_pair_name(source, target) == expected


def test_model_pair_name_unsupported_language_raises():
    with pytest.raises(UnsupportedLanguagePairError):
        model_pair_name("es", "fr")


def _train_spm(tmp_path, name: str) -> None:
    """Entrena un modelo SentencePiece real (corpus trivial) y lo deja en
    `tmp_path/{name}`, mismo nombre de archivo que espera `_MarianCT2Model`
    (`source.spm`/`target.spm`)."""
    import sentencepiece as spm

    corpus = tmp_path / "corpus.txt"
    corpus.write_text(
        "hola mundo como estas\nbuenos dias amigo\nesto es una prueba corta\n",
        encoding="utf-8",
    )
    prefix = tmp_path / name.replace(".spm", "")
    spm.SentencePieceTrainer.train(
        input=str(corpus), model_prefix=str(prefix), vocab_size=24,
        model_type="unigram",
    )
    (tmp_path / f"{prefix.name}.model").rename(tmp_path / name)


def test_default_translator_factory_real_ct2_sentencepiece_contract(tmp_path, monkeypatch):
    """Cierra la Desviación 3: `_default_translator_factory` debe devolver un
    objeto con `.translate(str) -> str`, no el `ctranslate2.Translator` crudo
    (que solo opera sobre listas de subtokens). Ejercita el contrato REAL de
    tokenización SentencePiece (entrenada de verdad, no un doble) contra un
    `ctranslate2.Translator` doblado (la inferencia CT2 en sí queda fuera:
    requeriría un modelo convertido real, inviable en un test unitario)."""
    import ctranslate2

    from ai_voice_interconnector.translation.model_loader import _default_translator_factory

    _train_spm(tmp_path, "source.spm")
    _train_spm(tmp_path, "target.spm")

    class _FakeCT2Translator:
        """Doble de ctranslate2.Translator: pasa los subtokens de entrada tal
        cual como hipótesis de salida (identidad), para poder verificar que
        el roundtrip encode->decode de SentencePiece es real de punta a punta."""

        def __init__(self, path):
            self.path = path

        def translate_batch(self, batch):
            class _Result:
                def __init__(self, tokens):
                    self.hypotheses = [tokens]
            return [_Result(tokens) for tokens in batch]

    monkeypatch.setattr(ctranslate2, "Translator", _FakeCT2Translator)

    model = _default_translator_factory(str(tmp_path))
    result = model.translate("hola mundo")

    assert isinstance(result, str)
    assert result.strip() != ""


def test_translate_appends_source_eos_token(tmp_path, monkeypatch):
    """Guarda de regresión del bug de EOS nunca emitido: los modelos
    Marian/opus-mt esperan el origen terminado en `</s>` (así lo antepone
    `MarianTokenizer` en HuggingFace), pero el conversor CT2 no lo añade
    automáticamente (`add_source_eos: false`). Sin este token el decoder
    entra en un loop de repetición y nunca termina (ver F5-remediacion.md).
    `_MarianCT2Model.translate` debe añadirlo antes de `translate_batch`."""
    import ctranslate2

    from ai_voice_interconnector.translation.model_loader import _default_translator_factory

    _train_spm(tmp_path, "source.spm")
    _train_spm(tmp_path, "target.spm")

    captured_batches = []

    class _CapturingCT2Translator:
        def __init__(self, path):
            self.path = path

        def translate_batch(self, batch):
            captured_batches.append(batch)

            class _Result:
                def __init__(self, tokens):
                    self.hypotheses = [tokens]
            return [_Result(tokens) for tokens in batch]

    monkeypatch.setattr(ctranslate2, "Translator", _CapturingCT2Translator)

    model = _default_translator_factory(str(tmp_path))
    model.translate("hola mundo")

    assert captured_batches[0][0][-1] == "</s>"
