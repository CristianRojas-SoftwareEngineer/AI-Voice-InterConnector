"""Tests para el gestor del ciclo de vida del daemon."""

import os
import pytest
import sys
import time
from pathlib import Path
from unittest.mock import patch, MagicMock

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from tts_sidecar.timing import SynthesisMetrics, SynthesisResult


class TestServerConcurrency:
    def test_health_responds_during_synthesis(self, tmp_path, monkeypatch):
        """Una síntesis bloqueada no debe congelar /health."""
        import threading
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices

        started = threading.Event()
        release = threading.Event()

        class SlowEngine:
            def speak(self, **kwargs):
                started.set()
                assert release.wait(timeout=10), "la síntesis nunca fue liberada"
                return SynthesisResult(
                    audio_bytes=b"RIFF" + b"\x00" * 40, metrics=SynthesisMetrics()
                )

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = SlowEngine()
        server.app.state.daemon.start_time = 0.0
        try:
            with TestClient(server.app) as client:
                result = {}

                def synth():
                    result["resp"] = client.post(
                        "/synthesize", json={"text": "hola", "voice": "crist"}
                    )

                t = threading.Thread(target=synth)
                t.start()
                assert started.wait(timeout=10), "la síntesis no arrancó"

                # Con la síntesis en curso, /health debe responder
                health = client.get("/health")
                assert health.status_code == 200

                release.set()
                t.join(timeout=10)
                assert result["resp"].status_code == 200
        finally:
            server.app.state.daemon.engine = old_engine


class TestServerAdmissionControl:
    """El semáforo de admisión acota las síntesis concurrentes admitidas."""

    def test_rejects_concurrent_request_when_saturated(self, tmp_path, monkeypatch):
        """Con el cupo agotado, una petición concurrente recibe 503 de inmediato."""
        import threading
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices

        started = threading.Event()
        release = threading.Event()

        class SlowEngine:
            def speak(self, **kwargs):
                started.set()
                assert release.wait(timeout=10), "la síntesis nunca fue liberada"
                return SynthesisResult(
                    audio_bytes=b"RIFF" + b"\x00" * 40, metrics=SynthesisMetrics()
                )

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))
        monkeypatch.setattr(server, "_admission_semaphore", threading.BoundedSemaphore(1))

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = SlowEngine()
        server.app.state.daemon.start_time = 0.0
        try:
            with TestClient(server.app) as client:
                result = {}

                def synth():
                    result["resp"] = client.post(
                        "/synthesize", json={"text": "hola", "voice": "crist"}
                    )

                t = threading.Thread(target=synth)
                t.start()
                assert started.wait(timeout=10), "la síntesis no arrancó"

                # Cupo agotado (BoundedSemaphore(1) ya tomado por la primera
                # síntesis en curso): la segunda petición se rechaza de inmediato.
                second = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert second.status_code == 503

                release.set()
                t.join(timeout=10)
                assert result["resp"].status_code == 200
        finally:
            server.app.state.daemon.engine = old_engine

    def test_permit_released_after_synthesis_completes(self, tmp_path, monkeypatch):
        """Al terminar la síntesis, el permiso se reintegra y una petición posterior responde 200."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices
        import threading

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))
        monkeypatch.setattr(server, "_admission_semaphore", threading.BoundedSemaphore(1))

        class FakeEngine:
            def speak(self, **kwargs):
                return SynthesisResult(
                    audio_bytes=b"RIFF" + b"\x00" * 40, metrics=SynthesisMetrics()
                )

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with TestClient(server.app) as client:
                first = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert first.status_code == 200

                second = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert second.status_code == 200
        finally:
            server.app.state.daemon.engine = old_engine

    def test_503_detail_is_actionable_without_system_paths(self, tmp_path, monkeypatch):
        """El 503 de saturación lleva un detail accionable y no filtra rutas del sistema."""
        import threading
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices

        started = threading.Event()
        release = threading.Event()

        class SlowEngine:
            def speak(self, **kwargs):
                started.set()
                assert release.wait(timeout=10), "la síntesis nunca fue liberada"
                return SynthesisResult(
                    audio_bytes=b"RIFF" + b"\x00" * 40, metrics=SynthesisMetrics()
                )

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))
        monkeypatch.setattr(server, "_admission_semaphore", threading.BoundedSemaphore(1))

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = SlowEngine()
        try:
            with TestClient(server.app) as client:
                def synth():
                    client.post(
                        "/synthesize", json={"text": "hola", "voice": "crist"}
                    )

                t = threading.Thread(target=synth)
                t.start()
                assert started.wait(timeout=10), "la síntesis no arrancó"

                resp = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert resp.status_code == 503
                detail = resp.json()["detail"]
                assert detail
                assert str(wav) not in resp.text

                release.set()
                t.join(timeout=10)
        finally:
            server.app.state.daemon.engine = old_engine


class TestKillPidVerified:
    def _fake_psutil(self, cmdline):
        proc = MagicMock()
        proc.cmdline.return_value = cmdline
        psutil_mock = MagicMock()
        psutil_mock.Process.return_value = proc
        return psutil_mock, proc

    def test_does_not_kill_foreign_processes(self, capsys):
        """Si otro servicio ocupa el puerto, no se le hace terminate()."""
        from tts_sidecar.daemon.daemon import DaemonManager

        psutil_mock, proc = self._fake_psutil(["node", "otro-servidor.js"])
        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            DaemonManager()._kill_pid(1234)

        proc.terminate.assert_not_called()
        assert "no parece ser el daemon" in capsys.readouterr().err

    def test_kills_own_daemon(self):
        from tts_sidecar.daemon.daemon import DaemonManager

        psutil_mock, proc = self._fake_psutil(
            ["python", "-m", "tts_sidecar.daemon.run"]
        )
        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            DaemonManager()._kill_pid(1234)

        proc.terminate.assert_called_once()


class TestStopDuringStartupWindow:
    """'daemon stop' durante la ventana de arranque (puerto cerrado)
    detecta el proceso por cmdline, avisa y devuelve False, sin matarlo."""

    def _manager_offline(self):
        """DaemonManager con health check negativo y puerto sin ocupar.

        Sin pidfile (`_read_pid` → None) para ejercitar el fallback por cmdline
        de forma determinista, con independencia de cualquier daemon.pid real.
        """
        from tts_sidecar.daemon.daemon import DaemonManager

        manager = DaemonManager()
        manager.is_running = lambda: False
        manager._get_pid_from_port = lambda: None
        manager._read_pid = lambda: None
        return manager

    def _psutil_with_processes(self, procs):
        psutil_mock = MagicMock()
        psutil_mock.process_iter.return_value = iter(procs)
        return psutil_mock

    def _proc(self, pid, cmdline):
        proc = MagicMock()
        proc.pid = pid
        proc.cmdline.return_value = cmdline
        return proc

    def test_starting_daemon_detected_returns_false_with_notice(self, capsys):
        manager = self._manager_offline()
        starting = self._proc(4321, ["python", "-m", "tts_sidecar.daemon.run"])
        psutil_mock = self._psutil_with_processes([starting])

        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            assert manager.stop() is False

        err = capsys.readouterr().err
        assert "arrancando" in err
        assert "4321" in err
        starting.terminate.assert_not_called()
        starting.kill.assert_not_called()

    def test_without_starting_daemon_keeps_current_behavior(self, capsys):
        manager = self._manager_offline()
        foreign = self._proc(777, ["node", "otro-servidor.js"])
        psutil_mock = self._psutil_with_processes([foreign])

        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            assert manager.stop() is True

        assert "no está corriendo" in capsys.readouterr().err

    def test_own_process_is_excluded_from_scan(self, capsys):
        """El marker podría aparecer en el cmdline del propio CLI: el escaneo
        excluye os.getpid() para no detectarse a sí mismo."""
        import os

        manager = self._manager_offline()
        own = self._proc(os.getpid(), ["python", "-m", "tts_sidecar.daemon.run"])
        psutil_mock = self._psutil_with_processes([own])

        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            assert manager.stop() is True

        assert "no está corriendo" in capsys.readouterr().err

    def test_generic_cli_cmdline_is_not_a_daemon_marker(self, capsys):
        """Otro comando del CLI ('tts-sidecar speech say') no debe confundirse
        con el daemon en arranque: solo cuentan los markers específicos."""
        manager = self._manager_offline()
        cli_proc = self._proc(555, ["tts-sidecar", "speech", "say", "--text", "hola"])
        psutil_mock = self._psutil_with_processes([cli_proc])

        with patch.dict(sys.modules, {"psutil": psutil_mock}):
            assert manager.stop() is True

        assert "no está corriendo" in capsys.readouterr().err


class TestDaemonManager:
    @patch("requests.get")
    def test_is_running_true(self, mock_get):
        from tts_sidecar.daemon import DaemonIPCClient
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        # Cuerpo válido de HealthResponse: la detección de vida ahora valida
        # identidad, no solo el status code.
        mock_resp.json.return_value = {
            "status": "healthy",
            "model_loaded": True,
            "uptime_seconds": 1.0,
        }
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        assert client.is_running() is True

    @patch("requests.get")
    def test_is_running_false_foreign_service_on_port(self, mock_get):
        """Un 200 de otro servicio en el puerto 8765, cuyo cuerpo no valida
        como HealthResponse, se trata como «no es el daemon»."""
        from tts_sidecar.daemon import DaemonIPCClient
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {"message": "soy otro servicio"}
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        assert client.is_running() is False

    @patch("requests.get")
    def test_is_running_false_non_json_body(self, mock_get):
        """Un 200 con cuerpo no-JSON tampoco cuenta como daemon vivo."""
        from tts_sidecar.daemon import DaemonIPCClient
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.side_effect = ValueError("no es JSON")
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        assert client.is_running() is False

    @patch("requests.get")
    def test_is_running_false_connection_error(self, mock_get):
        import requests
        from tts_sidecar.daemon import DaemonIPCClient
        mock_get.side_effect = requests.ConnectionError("refused")

        client = DaemonIPCClient()
        assert client.is_running() is False

    @patch("requests.get")
    def test_list_voices(self, mock_get):
        from tts_sidecar.daemon import DaemonIPCClient
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {"voices": ["crist", "testcli"]}
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        voices = client.list_voices()
        assert voices == ["crist", "testcli"]

    @patch("requests.get")
    def test_list_voices_on_error(self, mock_get):
        import requests
        from tts_sidecar.daemon import DaemonIPCClient
        mock_get.side_effect = requests.Timeout()

        client = DaemonIPCClient()
        voices = client.list_voices()
        assert voices == []

    @patch("requests.get")
    def test_list_voices_on_invalid_json(self, mock_get):
        """Cuerpo de éxito no conforme a VoicesResponse eleva DaemonIPCError."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.side_effect = ValueError("invalid json")
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no conforme"):
            client.list_voices()

    @patch("requests.get")
    def test_list_voices_on_non_conforming_body(self, mock_get):
        """Cuerpo 200 sin la clave 'voices' no valida el esquema → DaemonIPCError."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {"message": "otro servicio"}
        mock_get.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no conforme"):
            client.list_voices()

    @patch("requests.post")
    def test_precompute_voice_success(self, mock_post):
        from tts_sidecar.daemon import DaemonIPCClient
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {"name": "crist", "precomputed": True}
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        assert client.precompute_voice("crist") is True

    @patch("requests.post")
    def test_precompute_voice_http_error(self, mock_post):
        """Un 404 del daemon (voz inexistente) eleva DaemonIPCError con el detail."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError
        mock_resp = MagicMock()
        mock_resp.status_code = 404
        mock_resp.json.return_value = {"detail": "Voz no encontrada"}
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="Voz no encontrada"):
            client.precompute_voice("missing")

    @patch("requests.post")
    def test_precompute_voice_non_conforming_body(self, mock_post):
        """Cuerpo 200 sin las claves esperadas no valida → DaemonIPCError."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {"message": "otro servicio"}
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no conforme"):
            client.precompute_voice("crist")

    @patch("requests.post")
    def test_synthesize_success(self, mock_post):
        """El cliente reconstruye el WAV desde el frame `result` (base64) y
        reenvía cada frame `progress` (model_dump) a on_progress."""
        import base64
        import json
        from tts_sidecar.daemon import DaemonIPCClient
        from tts_sidecar.daemon.protocol import ProgressEvent

        audio = b"RIFF" + b"\x00" * 40
        lines = [
            json.dumps({"event": "progress", "stage": "conditionals"}).encode(),
            json.dumps({"event": "progress", "stage": "t3", "tokens": 20}).encode(),
            json.dumps({
                "event": "result",
                "audio_b64": base64.b64encode(audio).decode("ascii"),
                "t3_time": 9.7,
                "s3gen_time": 7.0,
            }).encode(),
        ]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        progreso = []
        client = DaemonIPCClient()
        result = client.synthesize(text="hola", voice="crist", on_progress=progreso.append)
        assert result.audio_bytes == audio
        assert result.metrics.t3 == 9.7
        assert result.metrics.s3gen == 7.0
        assert progreso == [
            ProgressEvent.model_validate(
                {"event": "progress", "stage": "conditionals"}
            ).model_dump(),
            ProgressEvent.model_validate(
                {"event": "progress", "stage": "t3", "tokens": 20}
            ).model_dump(),
        ]

    @patch("requests.post")
    def test_synthesize_error_frame(self, mock_post):
        """Un frame `error` del stream se convierte en DaemonIPCError."""
        import json
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [json.dumps({"event": "error", "detail": "internal error"}).encode()]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="Error del daemon: internal error"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_http_error_immediate(self, mock_post):
        """Un 400/503 de validación (respuesta inmediata, no stream) → DaemonIPCError."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        mock_resp = MagicMock()
        mock_resp.status_code = 400
        mock_resp.json.return_value = {"detail": "ruta no permitida"}
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="Error del daemon: ruta no permitida"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_without_result_frame_fails(self, mock_post):
        """Un stream que termina sin `result` ni `error` rompe el contrato → error."""
        import json
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [json.dumps({"event": "progress", "stage": "t3", "tokens": 10}).encode()]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no devolvió audio"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_non_json_line_raises(self, mock_post):
        """Una línea no-JSON en el stream eleva DaemonIPCError (sin tolerancia)."""
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [b"esto no es json"]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="línea no-JSON"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_unknown_event_raises(self, mock_post):
        """Un frame con `event` desconocido rompe el contrato → DaemonIPCError."""
        import json
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [json.dumps({"event": "telemetry", "cpu": 99}).encode()]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="desconocido"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_result_without_audio_raises(self, mock_post):
        """Un frame `result` sin `audio_b64` no valida el esquema → DaemonIPCError."""
        import json
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [json.dumps({"event": "result", "t3_time": 1.0, "s3gen_time": 2.0}).encode()]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no conforme"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_synthesize_result_invalid_base64_raises(self, mock_post):
        """Un `audio_b64` no base64 en el frame `result` eleva DaemonIPCError."""
        import json
        from tts_sidecar.daemon import DaemonIPCClient, DaemonIPCError

        lines = [json.dumps({
            "event": "result",
            "audio_b64": "!!!no es base64!!!",
            "t3_time": 1.0,
            "s3gen_time": 2.0,
        }).encode()]
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.iter_lines.return_value = iter(lines)
        mock_post.return_value = mock_resp

        client = DaemonIPCClient()
        with pytest.raises(DaemonIPCError, match="no decodificable"):
            client.synthesize(text="hola", voice="crist")

    @patch("requests.post")
    def test_stop_swallows_request_exception_and_reports_by_state(self, mock_post):
        """Un RequestException en el POST a /shutdown no revienta stop():
        se ignora y el resultado se decide por el estado real del proceso."""
        import requests
        from tts_sidecar.daemon.daemon import DaemonManager

        mock_post.side_effect = requests.RequestException("conexión rota")
        manager = DaemonManager()
        # Vivo al entrar (se intenta el cierre graceful); muerto después: stop()
        # debe reportar éxito pese al fallo HTTP, sin recurrir al kill por PID.
        with patch.object(manager, "is_running", side_effect=[True, False, False]):
            assert manager.stop() is True

    @patch("requests.get")
    def test_status_reports_unknown_on_request_exception(self, mock_get):
        """Si /health no responde pero el daemon parece vivo, status()
        devuelve el estado documentado "unknown" en lugar de propagar la excepción."""
        import requests
        from tts_sidecar.daemon.daemon import DaemonManager

        mock_get.side_effect = requests.RequestException("timeout")
        manager = DaemonManager()
        with patch.object(manager, "is_running", return_value=True):
            assert manager.status() == {"running": True, "status": "unknown"}


class TestSynthesizeStreaming:
    """El endpoint /synthesize emite NDJSON: N×progress → result, o error."""

    def _allowed_wav(self, tmp_path, monkeypatch):
        from tts_sidecar import voices

        allowed_root = tmp_path / "voices_permitido"
        allowed_root.mkdir()
        wav = allowed_root / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))
        return wav

    def test_order_progress_then_result(self, tmp_path, monkeypatch):
        import base64
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        wav = self._allowed_wav(tmp_path, monkeypatch)
        audio = b"RIFF" + b"\x00" * 40

        class FakeEngine:
            def speak(self, progress_callback=None, **kwargs):
                progress_callback({"event": "progress", "stage": "conditionals"})
                progress_callback({"event": "progress", "stage": "t3", "tokens": 10})
                return SynthesisResult(
                    audio_bytes=audio, metrics=SynthesisMetrics(t3=1.5, s3gen=2.5)
                )

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with TestClient(server.app) as client:
                resp = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert resp.status_code == 200
                assert resp.headers["content-type"].startswith("application/x-ndjson")
                lines = [json.loads(l) for l in resp.text.splitlines() if l.strip()]
                assert [l["event"] for l in lines] == ["progress", "progress", "result"]
                assert lines[0]["stage"] == "conditionals"
                assert lines[1]["tokens"] == 10
                assert base64.b64decode(lines[-1]["audio_b64"]) == audio
                assert lines[-1]["t3_time"] == 1.5
                assert lines[-1]["s3gen_time"] == 2.5
        finally:
            server.app.state.daemon.engine = old_engine

    def test_synthesis_error_emits_error_frame(self, tmp_path, monkeypatch):
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        wav = self._allowed_wav(tmp_path, monkeypatch)

        class FakeEngine:
            def speak(self, progress_callback=None, **kwargs):
                raise RuntimeError("boom interno con /ruta/secreta")

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with TestClient(server.app) as client:
                resp = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert resp.status_code == 200
                lines = [json.loads(l) for l in resp.text.splitlines() if l.strip()]
                assert lines[-1]["event"] == "error"
                # El detalle no filtra el mensaje/ruta interno real.
                assert lines[-1]["detail"] == "Error interno de síntesis"
                assert "secreta" not in resp.text
        finally:
            server.app.state.daemon.engine = old_engine


class TestDaemonStateInjection:
    """Los endpoints reciben el estado del daemon por inyección de
    dependencias (Depends(get_daemon_state)), no de globals de módulo. Se puede
    sustituir con app.dependency_overrides sin tocar app.state ni estado
    compartido — justo lo que un global mutable de módulo no permitía."""

    def test_health_uses_injected_state_via_dependency_override(self):
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        override_state = server.DaemonState(engine=MagicMock(), start_time=0.0)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                body = client.get("/health").json()
                assert body["model_loaded"] is True
                assert body["status"] == "healthy"
                from tts_sidecar import __version__
                assert body["version"] == __version__
        finally:
            server.app.dependency_overrides.clear()

    def test_synthesize_503_when_injected_state_has_no_engine(self):
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        override_state = server.DaemonState(engine=None)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/synthesize", json={"text": "hola", "voice": "crist"})
                assert resp.status_code == 503
        finally:
            server.app.dependency_overrides.clear()

    def test_precompute_voice_success(self):
        """El endpoint invoca engine.precompute_voice y devuelve precomputed=True."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        engine = MagicMock()
        override_state = server.DaemonState(engine=engine)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/voices/precompute", json={"name": "crist"})
                assert resp.status_code == 200
                assert resp.json() == {
                    "schema_version": "2",
                    "name": "crist",
                    "precomputed": True,
                }
            engine.precompute_voice.assert_called_once_with("crist")
        finally:
            server.app.dependency_overrides.clear()

    def test_precompute_voice_503_when_no_engine(self):
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        override_state = server.DaemonState(engine=None)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/voices/precompute", json={"name": "crist"})
                assert resp.status_code == 503
        finally:
            server.app.dependency_overrides.clear()

    def test_precompute_voice_404_when_voice_missing(self):
        """FileNotFoundError del engine se mapea a 404 sin filtrar rutas."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        engine = MagicMock()
        engine.precompute_voice.side_effect = FileNotFoundError(
            "Voz 'missing' no encontrada en /ruta/secreta"
        )
        override_state = server.DaemonState(engine=engine)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/voices/precompute", json={"name": "missing"})
                assert resp.status_code == 404
                assert resp.json()["detail"] == "Voz no encontrada"
                assert "secreta" not in resp.text
        finally:
            server.app.dependency_overrides.clear()

    def test_shutdown_releases_engine_on_injected_state(self):
        """/shutdown libera el engine y señaliza el server sobre el estado
        inyectado, sin mutar ningún global de módulo."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        fake_server = MagicMock()
        fake_server.should_exit = False
        override_state = server.DaemonState(engine=MagicMock(), server=fake_server)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/shutdown")
                assert resp.status_code == 200
            assert fake_server.should_exit is True
            assert override_state.engine is None
        finally:
            server.app.dependency_overrides.clear()

    def test_shutdown_503_when_no_server_registered(self):
        """Sin instancia de server registrada, /shutdown responde 503 (el kill
        por PID es la red de seguridad) en vez de intentar el apagado graceful."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        override_state = server.DaemonState(engine=MagicMock(), server=None)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/shutdown")
                assert resp.status_code == 503
        finally:
            server.app.dependency_overrides.clear()

    def test_list_voices_success(self):
        """/voices devuelve la lista que reporta el engine inyectado."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        engine = MagicMock()
        engine.list_voices.return_value = ["crist", "otra"]
        override_state = server.DaemonState(engine=engine)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.get("/voices")
                assert resp.status_code == 200
                assert resp.json() == {
                    "schema_version": "2",
                    "voices": ["crist", "otra"],
                }
        finally:
            server.app.dependency_overrides.clear()

    def test_list_voices_503_when_no_engine(self):
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        override_state = server.DaemonState(engine=None)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.get("/voices")
                assert resp.status_code == 503
        finally:
            server.app.dependency_overrides.clear()

    def test_precompute_voice_500_on_internal_error(self):
        """Un error genérico del engine se mapea a 500 sin filtrar el detalle."""
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        engine = MagicMock()
        engine.precompute_voice.side_effect = RuntimeError("fallo interno /ruta/secreta")
        override_state = server.DaemonState(engine=engine)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with TestClient(server.app) as client:
                resp = client.post("/voices/precompute", json={"name": "crist"})
                assert resp.status_code == 500
                assert resp.json()["detail"] == "Error interno de precómputo"
                assert "secreta" not in resp.text
        finally:
            server.app.dependency_overrides.clear()

    def test_synthesize_error_event_when_voice_resource_missing(self):
        """Si voice_paths lanza FileNotFoundError, el stream emite un evento
        error genérico sin filtrar la ruta interna real."""
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server

        engine = MagicMock()
        override_state = server.DaemonState(engine=engine)
        server.app.dependency_overrides[server.get_daemon_state] = lambda: override_state
        try:
            with patch.object(
                server.voices,
                "voice_paths",
                side_effect=FileNotFoundError("falta /ruta/secreta"),
            ):
                with TestClient(server.app) as client:
                    resp = client.post("/synthesize", json={"text": "hola", "voice": "crist"})
                    assert resp.status_code == 200
                    lines = [json.loads(l) for l in resp.text.splitlines() if l.strip()]
                    assert lines[-1]["event"] == "error"
                    assert lines[-1]["detail"] == "Recurso de voz no encontrado"
                    assert "secreta" not in resp.text
        finally:
            server.app.dependency_overrides.clear()


class TestDaemonStartLock:
    """El lock de arranque atómico (pidfile con O_EXCL) serializa los
    `start` concurrentes y reclama locks obsoletos."""

    def _manager(self, tmp_path):
        from tts_sidecar.daemon.daemon import DaemonManager

        manager = DaemonManager()
        pidfile = tmp_path / "daemon.pid"
        manager._pidfile = lambda: str(pidfile)
        return manager, pidfile

    def test_acquire_creates_lock_when_absent(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        assert manager._acquire_start_lock() is True
        assert pidfile.exists()

    def test_acquire_blocks_when_live_daemon_holds_lock(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        pidfile.write_text("4321", encoding="utf-8")
        manager._pid_alive_daemon = staticmethod(lambda pid: True)

        assert manager._acquire_start_lock() is False
        # El lock vigente no se toca.
        assert pidfile.read_text(encoding="utf-8") == "4321"

    def test_acquire_reclaims_dead_pid(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        pidfile.write_text("4321", encoding="utf-8")
        manager._pid_alive_daemon = staticmethod(lambda pid: False)

        assert manager._acquire_start_lock() is True
        # Reclamado y recreado vacío por el segundo open.
        assert pidfile.read_text(encoding="utf-8") == ""

    def test_acquire_reclaims_stale_empty_file(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        pidfile.write_text("", encoding="utf-8")
        old = time.time() - (manager.START_TIMEOUT + 60)
        os.utime(str(pidfile), (old, old))

        assert manager._acquire_start_lock() is True

    def test_acquire_keeps_recent_empty_file(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        pidfile.write_text("", encoding="utf-8")  # recién creado → arranque en curso

        assert manager._acquire_start_lock() is False

    def test_start_does_not_launch_when_lock_held(self, tmp_path):
        manager, _ = self._manager(tmp_path)
        manager.is_running = lambda: False
        manager._acquire_start_lock = lambda: False

        with patch("tts_sidecar.daemon.daemon.subprocess.Popen") as popen:
            assert manager.start() is True
            popen.assert_not_called()

    def test_start_writes_child_pid_after_popen(self, tmp_path):
        manager, pidfile = self._manager(tmp_path)
        manager.is_running = lambda: False
        manager._wait_for_ready = lambda: True
        fake_proc = MagicMock()
        fake_proc.pid = 4321

        with patch("tts_sidecar.daemon.daemon.subprocess.Popen", return_value=fake_proc):
            assert manager.start() is True

        assert pidfile.read_text(encoding="utf-8") == "4321"


class TestStopWithPidfile:
    """En la ventana de arranque, el pidfile es autoritativo y
    desambigua un daemon vivo (arrancando) de un zombie (PID muerto)."""

    def _offline(self, tmp_path):
        from tts_sidecar.daemon.daemon import DaemonManager

        manager = DaemonManager()
        manager.is_running = lambda: False
        manager._get_pid_from_port = lambda: None
        pidfile = tmp_path / "daemon.pid"
        manager._pidfile = lambda: str(pidfile)
        return manager, pidfile

    def test_live_daemon_in_pidfile_returns_false_with_notice(self, tmp_path, capsys):
        manager, pidfile = self._offline(tmp_path)
        pidfile.write_text("4321", encoding="utf-8")
        manager._pid_alive_daemon = staticmethod(lambda pid: True)

        assert manager.stop() is False
        err = capsys.readouterr().err
        assert "arrancando" in err
        assert "4321" in err
        # No se toca el pidfile de un daemon vivo.
        assert pidfile.exists()

    def test_dead_pid_in_pidfile_is_cleared_and_reports_not_running(self, tmp_path, capsys):
        manager, pidfile = self._offline(tmp_path)
        pidfile.write_text("4321", encoding="utf-8")
        manager._pid_alive_daemon = staticmethod(lambda pid: False)

        assert manager.stop() is True
        assert "no está corriendo" in capsys.readouterr().err
        # El pidfile obsoleto (zombie) se limpia.
        assert not pidfile.exists()


class TestRemoveOwnPidfile:
    """run.py borra su propio pidfile al cerrar, con guarda por PID."""

    def test_removes_pidfile_when_pid_matches(self, tmp_path, monkeypatch):
        from tts_sidecar.daemon import run

        pidfile = tmp_path / "daemon.pid"
        pidfile.write_text(str(os.getpid()), encoding="utf-8")
        monkeypatch.setattr("tts_sidecar.paths.daemon_pidfile", lambda: str(pidfile))

        run._remove_own_pidfile()
        assert not pidfile.exists()

    def test_keeps_pidfile_of_another_process(self, tmp_path, monkeypatch):
        from tts_sidecar.daemon import run

        pidfile = tmp_path / "daemon.pid"
        pidfile.write_text("999999", encoding="utf-8")
        monkeypatch.setattr("tts_sidecar.paths.daemon_pidfile", lambda: str(pidfile))

        run._remove_own_pidfile()
        assert pidfile.exists()


class TestServePortInUse:
    """El bind del puerto 8765 distingue EADDRINUSE y sale con
    EXIT_STATE_CONFLICT (6), sin reintentar ni reportar éxito (0)."""

    def _serve_that_fails_bind(self, errno_value, auto_restart=False):
        """Ejercita serve() con server.run() forzado a un OSError de bind.

        No carga el modelo real (get_instance mockeado) ni ocupa el puerto
        8765 (uvicorn.Server.run está parcheado para lanzar el error).
        """
        import errno
        from unittest.mock import MagicMock
        from tts_sidecar.daemon import run
        from tts_sidecar.cli import EXIT_ERROR

        with patch(
            "tts_sidecar.engine.ChatterboxEngine.get_instance",
            return_value=MagicMock(),
        ), patch(
            "tts_sidecar.compute_backend.ComputeBackendResolver.resolve",
            return_value="cpu",
        ), patch(
            "uvicorn.Server.run",
            side_effect=OSError(errno_value, "No se pudo enlazar el puerto"),
        ) as mock_run:
            with pytest.raises(SystemExit) as exc:
                run.serve(auto_restart=auto_restart)

        return exc, mock_run, EXIT_ERROR

    def test_eaddrinuse_posix_exits_with_port_in_use_code(self, capsys):
        import errno

        exc, mock_run, _ = self._serve_that_fails_bind(errno.EADDRINUSE)
        assert exc.value.code == 6
        mock_run.assert_called_once()
        err = capsys.readouterr().err
        assert "8765" in err
        assert "daemon stop" in err

    def test_wsaeaddrinuse_windows_exits_with_port_in_use_code(self, capsys):
        # WSAEADDRINUSE (Windows) == 10048
        exc, mock_run, _ = self._serve_that_fails_bind(10048)
        assert exc.value.code == 6
        mock_run.assert_called_once()
        err = capsys.readouterr().err
        assert "8765" in err
        assert "daemon stop" in err

    def test_eaddrinuse_with_auto_restart_does_not_retry(self, capsys):
        import errno

        exc, mock_run, _ = self._serve_that_fails_bind(
            errno.EADDRINUSE, auto_restart=True
        )
        # El bind fallido rompe el bucle de auto-reinicio de inmediato (exit 6),
        # sin recargar el modelo en vueltas sucesivas.
        assert exc.value.code == 6
        mock_run.assert_called_once()

    def test_other_oserror_exits_with_generic_error_code(self, capsys):
        import errno

        # Un OSError de binding distinto a EADDRINUSE (p.ej. EACCES) no debe
        # confundirse con «puerto en uso»: sale con EXIT_ERROR (1).
        exc, mock_run, EXIT_ERROR = self._serve_that_fails_bind(errno.EACCES)
        assert exc.value.code == EXIT_ERROR
        mock_run.assert_called_once()
        err = capsys.readouterr().err
        assert "no se pudo enlazar" in err


class TestSynthesisCancellation:
    """El worker aborta la síntesis al cancelarla el cliente.

    La cancelación es cooperativa: el closure ``push`` eleva
    ``SynthesisCancelled`` cuando el cliente se desconecta, y el engine la
    re-lanza (en vez de tragarla como las demás excepciones del callback). El
    worker la captura, no emite ``result``/``error`` y libera el semáforo.
    """

    def test_worker_aborts_when_progress_callback_signals_cancellation(self, tmp_path, monkeypatch):
        """Si el progress_callback eleva SynthesisCancelled, el stream termina
        sin frame result y el semáforo se libera (otra petición responde 200)."""
        import base64
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices
        from tts_sidecar.exceptions import SynthesisCancelled

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))

        class FakeEngine:
            def speak(self, progress_callback=None, **kwargs):
                progress_callback({"event": "progress", "stage": "conditionals"})
                progress_callback({"event": "progress", "stage": "t3", "tokens": 5})
                # El cliente se fue: señal cooperativa de cancelación.
                raise SynthesisCancelled()

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with TestClient(server.app) as client:
                resp = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert resp.status_code == 200
                lines = [json.loads(l) for l in resp.text.splitlines() if l.strip()]
                # Solo progress; sin result ni error.
                assert [l["event"] for l in lines] == ["progress", "progress"]
                assert all("result" != l["event"] for l in lines)

                # El semáforo se liberó: una segunda petición responde 200.
                second = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert second.status_code == 200
        finally:
            server.app.state.daemon.engine = old_engine

    def test_synthesis_completes_normally_without_cancellation(self, tmp_path, monkeypatch):
        """Regresión: una síntesis normal (sin cancelación) emite el frame result."""
        import base64
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))
        audio = b"RIFF" + b"\x00" * 40

        class FakeEngine:
            def speak(self, progress_callback=None, **kwargs):
                progress_callback({"event": "progress", "stage": "conditionals"})
                return SynthesisResult(
                    audio_bytes=audio, metrics=SynthesisMetrics(t3=1.0, s3gen=2.0)
                )

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with TestClient(server.app) as client:
                resp = client.post(
                    "/synthesize", json={"text": "hola", "voice": "crist"}
                )
                assert resp.status_code == 200
                lines = [json.loads(l) for l in resp.text.splitlines() if l.strip()]
                # No debe haber quedado como "interrumpida": hay result.
                result_frames = [l for l in lines if l["event"] == "result"]
                assert result_frames, "una síntesis sin cancelación debe emitir result"
                assert base64.b64decode(result_frames[-1]["audio_b64"]) == audio
        finally:
            server.app.state.daemon.engine = old_engine

    def test_client_disconnect_aborts_synthesis(self, tmp_path, monkeypatch):
        """Extremo a extremo: al desconectarse el cliente (GeneratorExit sobre el
        generador del stream, igual que lanza uvicorn en producción), el worker
        deja de síntetizar (contador < total) y no emite frame result.

        Nota: TestClient/httpx no entregan GeneratorExit cuando se cierra el
        stream (Starlette solo detecta desconexión si ``send`` levanta, que no
        ocurre en el transporte en memoria). Por eso se conduce el generador
        real ``event_stream`` devuelto por ``synthesize`` y se simula la
        desconexión con ``gen.close()`` — idéntico a lo que hace uvicorn en
        producción —, ejercitando así fielmente el handler de desconexión y el
        aborto cooperativo del worker.
        """
        import json
        import threading
        import time
        from tts_sidecar.daemon import server
        from tts_sidecar.daemon.protocol import SynthesizeRequest
        from tts_sidecar import voices

        wav = tmp_path / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))

        TOTAL = 50

        class FakeEngine:
            counter = 0

            def speak(self, progress_callback=None, **kwargs):
                # Bucle largo: cada iteración notifica progreso (y cede) para dar
                # al cliente tiempo de desconectarse. push aborta vía
                # SynthesisCancelled cuando cancel_event se activa.
                for i in range(TOTAL):
                    type(self).counter = i + 1
                    if progress_callback is not None:
                        progress_callback({"event": "progress", "stage": "t3", "tokens": i})
                    time.sleep(0.01)

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        # Captura el generador síncrono real (event_stream) en el momento en que
        # synthesize() construye el StreamingResponse: es exactamente el objeto
        # que produce la función y que uvicorn conduce en producción.
        captured = {}

        class _CaptureStreaming(server.StreamingResponse):
            def __init__(self, content, *args, **kwargs):
                captured["gen"] = content
                super().__init__(content, *args, **kwargs)

        monkeypatch.setattr(server, "StreamingResponse", _CaptureStreaming)
        try:
            req = SynthesizeRequest(text="hola", voice="crist")
            state = server.app.state.daemon
            # synthesize() valida la ruta, toma el semáforo y construye el
            # StreamingResponse (captura el generador event_stream).
            server.synthesize(req, state)
            gen = captured["gen"]
            # Avanza el generador: arranca el worker y produce la 1ª línea.
            first = next(gen)
            assert json.loads(first)["event"] == "progress"

            # El cliente se desconecta: close() lanza GeneratorExit sobre el
            # generador (igual que uvicorn en producción), que setea cancel_event.
            gen.close()

            # Espera a que el worker reaccione a la cancelación.
            time.sleep(0.5)

            # La síntesis se interrumpió: no llegó a completar las TOTAL
            # llamadas (no se emitió frame result).
            assert FakeEngine.counter < TOTAL
        finally:
            server.app.state.daemon.engine = old_engine


class TestDaemonMemoryClear:
    """El daemon libera la caché CUDA y fuerza GC tras cada síntesis."""

    def test_clear_model_memory_called_after_synthesis(self, tmp_path, monkeypatch):
        """Tras un POST /synthesize exitoso, _clear_model_memory se invoca exactamente una vez."""
        import json
        from fastapi.testclient import TestClient
        from tts_sidecar.daemon import server
        from tts_sidecar import voices
        from unittest.mock import patch, MagicMock

        allowed_root = tmp_path / "voices_permitido"
        allowed_root.mkdir()
        wav = allowed_root / "voz.wav"
        wav.write_bytes(b"RIFF\x00\x00\x00\x00WAVE")
        monkeypatch.setattr(voices, "voice_paths", lambda name: (str(wav), str(wav)))

        # Mock de la rutina de limpieza
        mock_clear = MagicMock()

        class FakeEngine:
            def speak(self, **kwargs):
                return SynthesisResult(
                    audio_bytes=b"RIFF" + b"\x00" * 40, metrics=SynthesisMetrics()
                )

        old_engine = server.app.state.daemon.engine
        server.app.state.daemon.engine = FakeEngine()
        try:
            with patch("tts_sidecar.daemon.server._clear_model_memory", mock_clear):
                with TestClient(server.app) as client:
                    resp = client.post(
                        "/synthesize", json={"text": "hola", "voice": "crist"}
                    )
                    assert resp.status_code == 200

            mock_clear.assert_called_once()
        finally:
            server.app.state.daemon.engine = old_engine

    def test_clear_model_memory_contract(self):
        """_clear_model_memory llama torch.cuda.empty_cache() y gc.collect()."""
        import sys
        from unittest.mock import MagicMock, patch
        from tts_sidecar.daemon import server

        # Mock de torch y gc
        mock_torch = MagicMock()
        mock_gc = MagicMock()

        with patch.dict(sys.modules, {"torch": mock_torch}):
            with patch.object(server, "gc", mock_gc):
                server._clear_model_memory()

        mock_torch.cuda.empty_cache.assert_called_once()
        mock_gc.collect.assert_called_once()

    def test_clear_model_memory_handles_missing_torch(self):
        """Si torch no está disponible, _clear_model_memory llama solo a gc.collect()."""
        import sys
        from unittest.mock import patch
        from tts_sidecar.daemon import server

        # Simular ausencia de torch
        with patch.dict(sys.modules, {"torch": None}):
            with patch.object(server, "gc") as mock_gc:
                server._clear_model_memory()
                mock_gc.collect.assert_called_once()
