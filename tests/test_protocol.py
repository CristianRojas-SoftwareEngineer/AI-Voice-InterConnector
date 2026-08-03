"""Tests para los modelos Pydantic de daemon/protocol.py."""

import pytest
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))
from tts_sidecar.daemon.protocol import (
    MAX_TEXT_LENGTH,
    MAX_VOICE_NAME_LENGTH,
    SynthesizeRequest,
    HealthResponse,
    VoicesResponse,
    ProgressEvent,
    ResultEvent,
    ErrorEvent,
)


class TestSynthesizeRequest:
    def test_valid_request(self):
        req = SynthesizeRequest(text="hola mundo", voice="crist")
        assert req.text == "hola mundo"
        assert req.voice == "crist"

    def test_full_request(self):
        req = SynthesizeRequest(text="test", voice="crist")
        assert req.text == "test"
        assert req.voice == "crist"

    def test_missing_text(self):
        with pytest.raises(ValueError):
            SynthesizeRequest(voice="crist")

    def test_missing_voice(self):
        with pytest.raises(ValueError):
            SynthesizeRequest(text="hola")

    def test_empty_text_rejected(self):
        with pytest.raises(ValueError):
            SynthesizeRequest(text="", voice="crist")

    def test_excessive_text_rejected(self):
        with pytest.raises(ValueError):
            SynthesizeRequest(text="a" * (MAX_TEXT_LENGTH + 1), voice="crist")

    def test_text_at_limit_accepted(self):
        req = SynthesizeRequest(text="a" * MAX_TEXT_LENGTH, voice="crist")
        assert len(req.text) == MAX_TEXT_LENGTH

    def test_protocol_without_model_or_compute_backend(self):
        campos = SynthesizeRequest.model_fields
        assert "model" not in campos
        assert "compute_backend" not in campos

    def test_excessive_voice_name_rejected(self):
        """El nombre de voz tiene tope de longitud."""
        excessive_name = "a" * (MAX_VOICE_NAME_LENGTH + 1)
        with pytest.raises(ValueError):
            SynthesizeRequest(text="hola", voice=excessive_name)

    def test_voice_name_at_limit_accepted(self):
        name = "a" * MAX_VOICE_NAME_LENGTH
        req = SynthesizeRequest(text="hola", voice=name)
        assert len(req.voice) == MAX_VOICE_NAME_LENGTH


class TestHealthResponse:
    def test_healthy_response(self):
        resp = HealthResponse(
            status="healthy", model_loaded={"es-latam": True}, uptime_seconds=10.5,
        )
        assert resp.status == "healthy"
        assert resp.model_loaded == {"es-latam": True}
        assert resp.uptime_seconds == 10.5

    def test_initializing_response(self):
        resp = HealthResponse(status="initializing", model_loaded={}, uptime_seconds=0.0)
        assert resp.status == "initializing"
        assert resp.model_loaded == {}

    def test_model_loaded_per_language(self):
        """model_loaded es una estructura por idioma (§3.11): qué modelos
        están calientes en RAM, no un booleano único."""
        resp = HealthResponse(
            status="healthy",
            model_loaded={"es-latam": True, "en": False},
            uptime_seconds=1.0,
        )
        assert resp.model_loaded["es-latam"] is True
        assert resp.model_loaded["en"] is False


class TestVoicesResponse:
    def test_voices_response(self):
        resp = VoicesResponse(voices=["crist", "testcli"])
        assert resp.voices == ["crist", "testcli"]

    def test_empty_voices(self):
        resp = VoicesResponse(voices=[])
        assert resp.voices == []


class TestStreamEvents:
    """Esquema NDJSON de /synthesize: progress / result / error."""

    def test_progress_event_defaults(self):
        ev = ProgressEvent()
        assert ev.event == "progress"
        assert ev.stage is None and ev.tokens is None and ev.elapsed is None

    def test_progress_event_with_tokens(self):
        ev = ProgressEvent(stage="t3", tokens=210)
        assert ev.event == "progress"
        assert ev.stage == "t3"
        assert ev.tokens == 210

    def test_progress_event_fixed_literal(self):
        """El discriminador `event` es un literal: no admite otros valores."""
        with pytest.raises(ValueError):
            ProgressEvent(event="result")

    def test_result_event(self):
        ev = ResultEvent(audio_b64="QUJD", t3_time=9.7, s3gen_time=7.0)
        assert ev.event == "result"
        assert ev.audio_b64 == "QUJD"
        assert ev.t3_time == 9.7
        assert ev.s3gen_time == 7.0

    def test_result_event_default_times(self):
        ev = ResultEvent(audio_b64="QUJD")
        assert ev.t3_time == 0.0 and ev.s3gen_time == 0.0

    def test_result_event_requires_audio(self):
        with pytest.raises(ValueError):
            ResultEvent()

    def test_error_event(self):
        ev = ErrorEvent(detail="Error interno de síntesis")
        assert ev.event == "error"
        assert ev.detail == "Error interno de síntesis"

    def test_error_event_requires_detail(self):
        with pytest.raises(ValueError):
            ErrorEvent()

    def test_json_serialization_includes_event(self):
        """Cada línea NDJSON lleva el discriminador `event` para el parseo del cliente."""
        import json

        assert json.loads(ProgressEvent(stage="t3", tokens=10).model_dump_json())["event"] == "progress"
        assert json.loads(ResultEvent(audio_b64="QUJD").model_dump_json())["event"] == "result"
        assert json.loads(ErrorEvent(detail="x").model_dump_json())["event"] == "error"


class TestUnicodeBoundaries:
    """Los topes de longitud (MAX_TEXT_LENGTH/MAX_VOICE_NAME_LENGTH) se
    validan en Pydantic v2 por longitud de la cadena Python (code points), no
    por bytes UTF-8: un acento o un emoji no deben contar doble."""

    def test_multibyte_text_at_limit_accepted(self):
        text = "ñ" * MAX_TEXT_LENGTH
        req = SynthesizeRequest(text=text, voice="crist")
        assert len(req.text) == MAX_TEXT_LENGTH

    def test_multibyte_text_over_limit_rejected(self):
        with pytest.raises(ValueError):
            SynthesizeRequest(text="ñ" * (MAX_TEXT_LENGTH + 1), voice="crist")

    def test_emoji_counts_as_single_codepoint(self):
        """Un emoji fuera del BMP es un solo code point en Python 3 (no un par
        de surrogates UTF-16): no debe contar doble contra el límite."""
        text = "😀" * MAX_TEXT_LENGTH
        req = SynthesizeRequest(text=text, voice="crist")
        assert len(req.text) == MAX_TEXT_LENGTH
        with pytest.raises(ValueError):
            SynthesizeRequest(text="😀" * (MAX_TEXT_LENGTH + 1), voice="crist")

    def test_unicode_voice_name_at_limit_accepted(self):
        name = "ñ" * MAX_VOICE_NAME_LENGTH
        req = SynthesizeRequest(text="hola", voice=name)
        assert len(req.voice) == MAX_VOICE_NAME_LENGTH

    def test_unicode_voice_name_over_limit_rejected(self):
        name = "ñ" * (MAX_VOICE_NAME_LENGTH + 1)
        with pytest.raises(ValueError):
            SynthesizeRequest(text="hola", voice=name)

    def test_text_with_accents_and_spanish_punctuation_roundtrips(self):
        text = "¿Cómo estás? ¡Qué bien! Ñoño güiro."
        req = SynthesizeRequest(text=text, voice="crist")
        assert req.text == text


class TestUnicodeInStreamEvents:
    """Los eventos NDJSON (progress/result/error) también llevan texto libre
    (stage, detail): deben preservar unicode sin normalizarlo ni truncarlo."""

    def test_progress_event_stage_with_unicode(self):
        ev = ProgressEvent(stage="síntesis-de-audio", tokens=1)
        assert ev.stage == "síntesis-de-audio"

    def test_error_event_detail_with_unicode_and_emoji(self):
        ev = ErrorEvent(detail="Error de síntesis: voz no encontrada 🎙️")
        assert "síntesis" in ev.detail
        assert "🎙️" in ev.detail

    def test_error_event_detail_serializes_unicode_correctly_in_json(self):
        import json

        ev = ErrorEvent(detail="No se pudo cargar la voz «default»")
        payload = json.loads(ev.model_dump_json())
        assert payload["detail"] == "No se pudo cargar la voz «default»"


class TestProtocolVersioning:
    """Los 5 modelos del protocolo heredan de ProtocolModel: schema_version
    fijo, y extra="ignore" para tolerar el skew de versiones daemon↔cliente."""

    MODELS_WITH_REQUIRED = {
        ProgressEvent: {},
        ResultEvent: {"audio_b64": "QUJD"},
        ErrorEvent: {"detail": "x"},
        HealthResponse: {"status": "healthy", "model_loaded": {"es-latam": True}, "uptime_seconds": 1.0},
        VoicesResponse: {"voices": []},
    }

    def test_all_models_declare_schema_version_3(self):
        import json

        for model_cls, kwargs in self.MODELS_WITH_REQUIRED.items():
            payload = json.loads(model_cls(**kwargs).model_dump_json())
            assert payload["schema_version"] == "3", model_cls.__name__

    def test_unknown_extra_fields_are_ignored_forward_skew(self):
        """Un daemon más nuevo que envía un campo desconocido no rompe un
        cliente viejo: extra="ignore" lo descarta en vez de fallar."""
        for model_cls, kwargs in self.MODELS_WITH_REQUIRED.items():
            instance = model_cls(**kwargs, campo_del_futuro="algo", otro_campo=123)
            assert not hasattr(instance, "campo_del_futuro")

    def test_missing_schema_version_and_version_default_backward_skew(self):
        """Un daemon viejo que no puebla schema_version/version no rompe un
        cliente nuevo: los defaults completan sin exigir el campo."""
        health = HealthResponse.model_validate({
            "status": "healthy", "model_loaded": {"es-latam": True}, "uptime_seconds": 1.0,
        })
        assert health.schema_version == "3"
        assert health.version == ""

    def test_health_response_carries_version(self):
        resp = HealthResponse(
            status="healthy", model_loaded={"es-latam": True}, uptime_seconds=1.0, version="0.6.0",
        )
        assert resp.version == "0.6.0"
