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
    from tts_sidecar.engine import ChatterboxEngine
    from tts_sidecar.conditionals import ConditionalsPreparer
    from tts_sidecar.audio_writer import AudioWriter
    from tts_sidecar.synthesis import SynthesisOrchestrator

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


class TestSpeakProgressCallback:
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
        from tts_sidecar.engine import ChatterboxEngine

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
        from tts_sidecar.engine import ChatterboxEngine

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
        with caplog.at_level(logging.DEBUG, logger="tts_sidecar.engine"):
            eng._emit_progress(stage="tts")  # no debe lanzar

        matching = [r for r in caplog.records if "callback de progreso" in r.message.lower()]
        assert matching, "el swallow debe registrar un debug"
        assert any(r.exc_info for r in matching), "debe incluir la traza (exc_info)"

    def test_token_counting_raising_cb_swallowed_and_logged(self, caplog):
        import logging
        from tts_sidecar.engine import ChatterboxEngine

        def boom(ev):
            raise RuntimeError("cb roto")

        with caplog.at_level(logging.DEBUG, logger="tts_sidecar.engine"):
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
        from tts_sidecar.exceptions import SynthesisCancelled

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
        from tts_sidecar.engine import ChatterboxEngine
        from tts_sidecar.exceptions import SynthesisCancelled

        def boom_cancel(ev):
            raise SynthesisCancelled()

        with pytest.raises(SynthesisCancelled):
            list(ChatterboxEngine._token_counting_iter(range(100), boom_cancel))

        def boom_other(ev):
            raise ValueError("error del callback")

        # Otra excepción del callback no debe interrumpir la iteración.
        salida = list(ChatterboxEngine._token_counting_iter(range(100), boom_other))
        assert salida == list(range(100))


class TestTokenShimInstall:
    def test_shim_wraps_sampling_tqdm(self, monkeypatch):
        """Instalado el shim, un tqdm(desc='Sampling') con callback activo cuenta
        tokens; sin callback delega en el tqdm real."""
        from tts_sidecar.engine import ChatterboxEngine
        from chatterbox.models.t3 import t3 as t3_mod

        eng = ChatterboxEngine.__new__(ChatterboxEngine)
        eng._active_progress_cb = None

        # Restaura el símbolo real al terminar para no contaminar otros tests.
        real_tqdm = t3_mod.tqdm
        monkeypatch.setattr(t3_mod, "tqdm", real_tqdm, raising=False)

        eng._install_token_progress_shim()
        assert getattr(t3_mod.tqdm, "_is_tts_sidecar_shim", False)

        eventos = []
        eng._active_progress_cb = lambda ev: eventos.append(ev)
        salida = list(t3_mod.tqdm(range(50), desc="Sampling", dynamic_ncols=True))
        assert salida == list(range(50))
        assert eventos, "con callback activo y desc='Sampling' debe contar tokens"

        # Sin callback: delega en el tqdm real y solo itera.
        eng._active_progress_cb = None
        assert list(t3_mod.tqdm(range(5), desc="Sampling")) == list(range(5))
