"""Tests para SynthesisOrchestrator.

Validan el flujo extraído de `ChatterboxEngine.synthesize`: emisión de etapas,
ciclo de vida de `_active_progress_cb` (limpieza en finally,
un callback roto no aborta), memoización de conditionals por mtime y recomputo
ante un `conditionals.pt` corrupto. Usan dobles ligeros del motor (sin cargar el
modelo real) y ejercen el orquestador directamente.
"""

import sys
import wave
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from tts_sidecar.audio_writer import AudioWriter
from tts_sidecar.conditionals import ConditionalsPreparer
from tts_sidecar.engine import ChatterboxEngine
from tts_sidecar.synthesis import SynthesisOrchestrator


def _engine_with_orchestrator():
    """ChatterboxEngine sin cargar el modelo real, con orquestador cableado."""
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
    eng._orchestrator = SynthesisOrchestrator(eng, eng._conditionals_prep, AudioWriter())
    return eng


class TestOrchestratorSynthesize:
    def test_emits_stage_events(self, tmp_path):
        eng = _engine_with_orchestrator()
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        events = []
        eng._orchestrator.synthesize(
            "hola", None, str(speech), lambda ev: events.append(ev)
        )

        stages = [ev["stage"] for ev in events]
        assert stages == ["conditionals", "tts", "encoding"]
        assert all(ev["event"] == "progress" for ev in events)

    def test_returns_valid_wav_bytes(self, tmp_path):
        eng = _engine_with_orchestrator()
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        result = eng._orchestrator.synthesize("hola", None, str(speech), None)
        assert isinstance(result.audio_bytes, bytes)
        with wave.open(__import__("io").BytesIO(result.audio_bytes), 'rb') as wf:
            assert wf.getnchannels() == 1
            assert wf.getframerate() == 24000

    def test_callback_is_cleared_in_finally(self, tmp_path):
        eng = _engine_with_orchestrator()
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng._orchestrator.synthesize(
            "hola", None, str(speech), lambda ev: None
        )
        assert eng._active_progress_cb is None

    def test_callback_exception_does_not_break_synthesis(self, tmp_path):
        eng = _engine_with_orchestrator()
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        def boom(ev):
            raise RuntimeError("callback roto")

        result = eng._orchestrator.synthesize("hola", None, str(speech), boom)
        assert isinstance(result.audio_bytes, bytes)
        assert eng._active_progress_cb is None


class TestOrchestratorConditionals:
    def test_recomputes_with_corrupt_conditionals(self, tmp_path):
        eng = _engine_with_orchestrator()
        voice = tmp_path / "voz"
        voice.mkdir()
        (voice / "conditionals.pt").write_bytes(b"basura")
        speech = voice / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        recomputos = []
        eng._conditionals_prep.compute = lambda *a, **kw: recomputos.append(kw)

        eng._orchestrator.synthesize("hola", None, str(speech), None)
        assert recomputos, "el orquestador debe recomputar cuando conditionals.pt es corrupto"

    def test_memoization_by_mtime(self, tmp_path):
        eng = _engine_with_orchestrator()
        voice = tmp_path / "voz"
        voice.mkdir()
        conds = voice / "conditionals.pt"
        conds.write_bytes(b"valido")
        speech = voice / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        loads = []
        def fake_load(voice_dir):
            loads.append(voice_dir)
            eng._tts.conds = object()  # el método real fija tts.conds al cargar
            return True
        eng.load_precomputed_conditionals = fake_load
        eng._conditionals_prep.compute = lambda *a, **kw: None

        eng._orchestrator.synthesize("hola", None, str(speech), None)
        eng._orchestrator.synthesize("hola otra vez", None, str(speech), None)
        assert len(loads) == 1, "la segunda síntesis de la misma voz no debe releer disco"

        # Un conditionals.pt regenerado (mtime nuevo) invalida la memoización
        mtime = conds.stat().st_mtime + 10
        import os
        os.utime(conds, (mtime, mtime))
        eng._orchestrator.synthesize("hola de nuevo", None, str(speech), None)
        assert len(loads) == 2


class TestOrchestratorLanguageRouting:
    """Ramificación de generate() por idioma (rediseño cross-lingual)."""

    def _engine_with_recording_tts(self, language):
        eng = _engine_with_orchestrator()
        eng._language = language
        calls = []

        class RecordingTTS:
            conds = None
            sr = 24000

            def generate(self, text, **kwargs):
                calls.append(kwargs)
                return [0.0]

        eng._tts = RecordingTTS()
        eng._orchestrator = SynthesisOrchestrator(eng, eng._conditionals_prep, AudioWriter())
        return eng, calls

    def test_es_mx_latam_passes_language_id_and_route_defaults(self, tmp_path):
        eng, calls = self._engine_with_recording_tts("es-mx-latam")
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng._orchestrator.synthesize("hola", None, str(speech), None)

        assert calls[0]["language_id"] == "es"
        assert calls[0]["exaggeration"] == 0.75
        assert calls[0]["cfg_weight"] == 0.5
        assert calls[0]["temperature"] == 0.8

    def test_en_omits_language_id_and_uses_route_defaults(self, tmp_path):
        eng, calls = self._engine_with_recording_tts("en")
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng._orchestrator.synthesize("hello", None, str(speech), None)

        assert "language_id" not in calls[0]
        assert calls[0]["exaggeration"] == 0.65
        assert calls[0]["cfg_weight"] == 0.3
        assert calls[0]["temperature"] == 0.7

    def test_explicit_overrides_win_over_route_defaults(self, tmp_path):
        eng, calls = self._engine_with_recording_tts("en")
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng._orchestrator.synthesize(
            "hello",
            None,
            str(speech),
            None,
            exaggeration=0.9,
            cfg_weight=0.1,
            temperature=0.6,
        )

        assert calls[0]["exaggeration"] == 0.9
        assert calls[0]["cfg_weight"] == 0.1
        assert calls[0]["temperature"] == 0.6

    def test_missing_language_attribute_defaults_to_es_mx_latam(self, tmp_path):
        """Un engine sin `_language` (dobles de tests preexistentes) conserva el
        comportamiento legacy: language_id="es" y defaults de es-mx-latam."""
        eng = _engine_with_orchestrator()
        assert not hasattr(eng, "_language")
        calls = []

        class RecordingTTS:
            conds = None
            sr = 24000

            def generate(self, text, **kwargs):
                calls.append(kwargs)
                return [0.0]

        eng._tts = RecordingTTS()
        eng._orchestrator = SynthesisOrchestrator(eng, eng._conditionals_prep, AudioWriter())
        eng._conditionals_prep.compute = lambda *a, **kw: None
        speech = tmp_path / "speech-reference.wav"
        speech.write_bytes(b"RIFF")

        eng._orchestrator.synthesize("hola", None, str(speech), None)

        assert calls[0]["language_id"] == "es"
        assert calls[0]["exaggeration"] == 0.75


class TestOrchestratorExposesAudioWriter:
    def test_audio_writer_reachable_for_test_double(self):
        eng = _engine_with_orchestrator()
        # Los tests sustituyen este write por un doble (ver test_engine_cache/progress).
        assert eng._orchestrator.audio_writer is not None
        assert hasattr(eng._orchestrator.audio_writer, "write")
