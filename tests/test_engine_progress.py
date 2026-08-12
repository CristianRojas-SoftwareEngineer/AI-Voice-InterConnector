"""Tests del progreso de síntesis del motor (Fase 2).

Cubren:
  - synthesize(progress_callback=...) emite los eventos de etapa esperados.
  - El shim de conteo de tokens (_token_counting_iter) reporta iteraciones con
    throttle sobre un iterable falso, sin romper la iteración.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))


def _engine_stub(tmp_path):
    """ChatterboxEngine sin cargar el modelo real (bypass de __init__)."""
    from ai_voice_interconnector.engine import ChatterboxEngine
    from ai_voice_interconnector.conditionals import ConditionalsPreparer
    from ai_voice_interconnector.audio_writer import AudioWriter
    from ai_voice_interconnector.synthesis import SynthesisOrchestrator

    eng = ChatterboxEngine.__new__(ChatterboxEngine)
    eng.compute_backend = "cpu"
    eng._conds_cache_key = None
    eng._active_progress_cb = None
    eng._conditionals_prep = ConditionalsPreparer()

    class FakeTTS:
        conds = None
        sr = 24000

        def generate(self, text, **kwargs):
            return [0.0]

    eng._tts = FakeTTS()
    # synthesize() delega en el orquestador; lo cableamos igual que __init__.
    eng._audio_writer = AudioWriter()
    eng._orchestrator = SynthesisOrchestrator(
        eng, eng._conditionals_prep, eng._audio_writer
    )
    return eng


class TestSynthesizeProgressCallback:
    def test_emits_stage_events(self, tmp_path, monkeypatch):
        eng = _engine_stub(tmp_path)
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )
        eng._conditionals_prep.compute = lambda *a, **kw: None

        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        events = []
        eng.synthesize(
            "hola",
            speech_reference=str(speech),
            progress_callback=lambda ev: events.append(ev),
        )

        stages = [ev["stage"] for ev in events]
        # generate() de FakeTTS no pasa por los wrappers timed_t3/timed_s3gen,
        # así que aquí verificamos las etapas emitidas directamente por synthesize().
        assert stages == ["conditionals", "tts", "encoding"]
        assert all(ev["event"] == "progress" for ev in events)

    def test_callback_is_cleared_in_finally(self, tmp_path, monkeypatch):
        eng = _engine_stub(tmp_path)
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng.synthesize("hola", speech_reference=str(speech), progress_callback=lambda ev: None)
        assert eng._active_progress_cb is None

    def test_callback_exception_does_not_break_synthesis(self, tmp_path, monkeypatch):
        eng = _engine_stub(tmp_path)
        monkeypatch.setattr(
            eng._orchestrator.audio_writer, "write",
            lambda audio_data, sample_rate: b"RIFF",
        )
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        def boom(ev):
            raise RuntimeError("callback roto")

        assert eng.synthesize("hola", speech_reference=str(speech), progress_callback=boom).audio_bytes == b"RIFF"


class TestTokenCountingIter:
    def test_reports_iterations_with_throttle(self):
        from ai_voice_interconnector.engine import ChatterboxEngine

        eventos = []
        # 100 iteraciones: con throttle de ~10 tokens el conteo reportado avanza
        # en múltiplos de 10 (el primer emit es en count==10).
        salida = list(
            ChatterboxEngine._token_counting_iter(range(100), lambda ev: eventos.append(ev))
        )

        # La iteración es transparente: yield de todos los elementos.
        assert salida == list(range(100))
        assert eventos, "el shim debe reportar al menos un evento de tokens"
        assert all(ev["stage"] == "t3" and ev["event"] == "progress" for ev in eventos)
        assert all(ev["tokens"] % 10 == 0 for ev in eventos)
        # El conteo es ascendente.
        counts = [ev["tokens"] for ev in eventos]
        assert counts == sorted(counts)

    def test_empty_iterable_emits_nothing(self):
        from ai_voice_interconnector.engine import ChatterboxEngine

        eventos = []
        salida = list(
            ChatterboxEngine._token_counting_iter([], lambda ev: eventos.append(ev))
        )
        assert salida == []
        assert eventos == []


class TestSilentExceptionLogging:
    """Los swallows inocuos dejan traza a nivel debug sin propagar.

    Antes eran `except Exception: pass` mudos; ahora emiten `logger.debug(...,
    exc_info=True)` para que la degradación sea diagnosticable, conservando la
    supresión (un callback roto no aborta la síntesis).
    """

    def test_emit_progress_swallows_and_logs(self, tmp_path, caplog):
        import logging

        eng = _engine_stub(tmp_path)

        def boom(ev):
            raise RuntimeError("cb roto")

        eng._active_progress_cb = boom
        with caplog.at_level(logging.DEBUG, logger="ai_voice_interconnector.engine"):
            eng._emit_progress(stage="tts")  # no debe lanzar

        matching = [r for r in caplog.records if "callback de progreso" in r.message.lower()]
        assert matching, "el swallow debe registrar un debug"
        assert any(r.exc_info for r in matching), "debe incluir la traza (exc_info)"

    def test_token_counting_raising_cb_swallowed_and_logged(self, caplog):
        import logging
        from ai_voice_interconnector.engine import ChatterboxEngine

        def boom(ev):
            raise RuntimeError("cb roto")

        with caplog.at_level(logging.DEBUG, logger="ai_voice_interconnector.engine"):
            salida = list(ChatterboxEngine._token_counting_iter(range(100), boom))

        # La iteración no se interrumpe pese al callback roto.
        assert salida == list(range(100))
        assert any(
            "tokens" in r.message.lower() and r.exc_info for r in caplog.records
        ), "el callback roto de tokens debe registrar un debug con traza"


class TestSynthesisCancelledPropagation:
    """El engine deja propagar ``SynthesisCancelled`` desde los
    callbacks de progreso, pero sigue tragando cualquier otra excepción del
    callback (contrato best-effort)."""

    def test_emit_progress_propagates_cancellation_but_swallows_other_errors(self, tmp_path):
        from ai_voice_interconnector.exceptions import SynthesisCancelled

        eng = _engine_stub(tmp_path)

        def boom_cancel(ev):
            raise SynthesisCancelled()

        eng._active_progress_cb = boom_cancel
        with pytest.raises(SynthesisCancelled):
            eng._emit_progress(stage="t3")

        def boom_other(ev):
            raise ValueError("error del callback")

        eng._active_progress_cb = boom_other
        # Otra excepción del callback no debe propagarse (se traga).
        eng._emit_progress(stage="t3")

    def test_token_counting_iter_propagates_cancellation(self):
        from ai_voice_interconnector.engine import ChatterboxEngine
        from ai_voice_interconnector.exceptions import SynthesisCancelled

        def boom_cancel(ev):
            raise SynthesisCancelled()

        with pytest.raises(SynthesisCancelled):
            list(ChatterboxEngine._token_counting_iter(range(100), boom_cancel))

        def boom_other(ev):
            raise ValueError("error del callback")

        # Otra excepción del callback no debe interrumpir la iteración.
        salida = list(ChatterboxEngine._token_counting_iter(range(100), boom_other))
        assert salida == list(range(100))


class TestAlignmentHookCleanup:
    """El wrapper timed_t3 vacía los forward_hooks que T3.inference deja tras
    cada llamada, evitando la acumulación no acotada en el daemon de larga vida.

    En el modelo multilingüe, el cuerpo de T3.inference reconstruye en CADA
    llamada un AlignmentStreamAnalyzer cuyo constructor registra un forward_hook
    por capa en LLAMA_ALIGNED_HEADS = [(12,15),(13,11),(9,2)] sin guardar el
    handle ni removerlo nunca. Como el motor se cachea entre peticiones, esos
    hooks crecen a 3·N con N síntesis. Aquí se dobla ese defecto con módulos
    torch reales y se verifica que, tras el fix, el conteo queda acotado a 0.
    """

    _ALIGNED = {9, 12, 13}

    @staticmethod
    def _fake_tts():
        import types

        import torch

        class _Attn(torch.nn.Module):
            def forward(self, x):  # pragma: no cover - nunca se invoca
                return x

        class _Layer(torch.nn.Module):
            def __init__(self):
                super().__init__()
                self.self_attn = _Attn()

        layers = [_Layer() for _ in range(16)]

        class _HP:
            is_multilingual = True

        class _T3:
            def __init__(self):
                self.hp = _HP()
                self.tfmr = types.SimpleNamespace(layers=layers)

            def inference(self, *a, **kw):
                # Imita el defecto: un forward_hook por capa alineada, sin remover.
                for idx in TestAlignmentHookCleanup._ALIGNED:
                    self.tfmr.layers[idx].self_attn.register_forward_hook(
                        lambda *a, **kw: None
                    )
                return object()

        tts = types.SimpleNamespace(
            t3=_T3(),
            s3gen=types.SimpleNamespace(inference=lambda *a, **kw: object()),
            watermarker=types.SimpleNamespace(apply_watermark=lambda *a, **kw: None),
        )
        return tts, layers

    def _engine(self, monkeypatch):
        from ai_voice_interconnector.engine import ChatterboxEngine
        from chatterbox.models.t3 import t3 as t3_mod

        # _apply_synthesis_optimizations instala el shim global de tqdm (símbolo
        # de chatterbox.models.t3.t3). Tomamos snapshot para que monkeypatch lo
        # restaure al terminar y no contamine a TestTokenShimInstall.
        monkeypatch.setattr(t3_mod, "tqdm", t3_mod.tqdm, raising=False)

        eng = ChatterboxEngine.__new__(ChatterboxEngine)
        eng._active_progress_cb = None
        tts, layers = self._fake_tts()
        eng._tts = tts
        eng._apply_synthesis_optimizations()
        return eng, layers

    def test_hooks_do_not_accumulate_across_calls(self, monkeypatch):
        eng, layers = self._engine(monkeypatch)

        # Dos síntesis a través del wrapper: sin el fix, los hooks crecerían a
        # 3·N (3 tras la 1ª, 6 tras la 2ª); con el fix quedan en 0 tras cada una.
        for _ in range(2):
            eng._tts.t3.inference()
            for idx in self._ALIGNED:
                assert len(layers[idx].self_attn._forward_hooks) == 0

    def test_english_only_path_is_untouched(self, monkeypatch):
        eng, layers = self._engine(monkeypatch)
        eng._tts.t3.hp.is_multilingual = False

        # En la ruta english_only el analizador nunca registra estos hooks; el
        # doble sí los registra, pero la limpieza no debe actuar (no es su ruta).
        eng._tts.t3.inference()
        total = sum(len(layers[idx].self_attn._forward_hooks) for idx in self._ALIGNED)
        assert total == 3


class TestTokenShimInstall:
    def test_shim_wraps_sampling_tqdm(self, monkeypatch):
        """Instalado el shim, un tqdm(desc='Sampling') con callback activo cuenta
        tokens; sin callback delega en el tqdm real."""
        from ai_voice_interconnector.engine import ChatterboxEngine
        from chatterbox.models.t3 import t3 as t3_mod

        eng = ChatterboxEngine.__new__(ChatterboxEngine)
        eng._active_progress_cb = None

        # Restaura el símbolo real al terminar para no contaminar otros tests.
        real_tqdm = t3_mod.tqdm
        monkeypatch.setattr(t3_mod, "tqdm", real_tqdm, raising=False)

        eng._install_token_progress_shim()
        assert getattr(t3_mod.tqdm, "_is_ai_voice_interconnector_shim", False)

        eventos = []
        eng._active_progress_cb = lambda ev: eventos.append(ev)
        salida = list(t3_mod.tqdm(range(50), desc="Sampling", dynamic_ncols=True))
        assert salida == list(range(50))
        assert eventos, "con callback activo y desc='Sampling' debe contar tokens"

        # Sin callback: delega en el tqdm real y solo itera.
        eng._active_progress_cb = None
        assert list(t3_mod.tqdm(range(5), desc="Sampling")) == list(range(5))
