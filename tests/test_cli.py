"""Tests para los comandos del CLI."""

import os
import pytest
import sys
import warnings
from pathlib import Path
from unittest.mock import patch, MagicMock

from tts_sidecar.exit_codes import CliError, EXIT_NOT_APPLICABLE, EXIT_STATE_CONFLICT

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

import tempfile

# Directorio temporal compartido para los .wav de prueba que deben existir en
# disco el cliente ahora valida existencia/extensión antes del despacho.
_VOICE_TMP = tempfile.mkdtemp(prefix="tts-sidecar-cli-")


def _make_wav(name):
    """Crea (si falta) un .wav existente con cabecera RIFF/WAVE mínima y retorna su ruta."""
    path = os.path.join(_VOICE_TMP, name)
    if not os.path.exists(path):
        with open(path, "wb") as f:
            f.write(b"RIFF\x24\x00\x00\x00WAVE")
    return path


def _synth_result(audio_bytes=b"RIFF", t3=0.0, s3gen=0.0):
    """Fixture de `SynthesisResult`, el retorno real de engine.synthesize()/client.synthesize()."""
    from tts_sidecar.timing import SynthesisMetrics, SynthesisResult
    return SynthesisResult(audio_bytes=audio_bytes, metrics=SynthesisMetrics(t3=t3, s3gen=s3gen))


class MockArgs:
    def __init__(self, **kwargs):
        self.text = kwargs.get("text", "test text")
        self.voice = kwargs.get("voice", None)
        self.model = kwargs.get("model", "es-mx-latam")
        self.compute_backend = kwargs.get("compute_backend", "auto")
        self.name = kwargs.get("name", "testcli")
        self.timbre_reference = kwargs.get("timbre_reference", "timbre.wav")
        self.speech_reference = kwargs.get("speech_reference", "speech.wav")
        self.daemon = kwargs.get("daemon", False)
        self.no_daemon = kwargs.get("no_daemon", False)
        self.json = kwargs.get("json", False)
        self.remove_path = kwargs.get("remove_path", False)
        self.force_update = kwargs.get("force_update", False)
        self.uninstall = kwargs.get("uninstall", False)
        self.yes = kwargs.get("yes", False)
        self.language = kwargs.get("language", "all")
        self.exaggeration = kwargs.get("exaggeration", None)
        self.cfg_weight = kwargs.get("cfg_weight", None)
        self.temperature = kwargs.get("temperature", None)
        self.all = kwargs.get("all", False)
        # Campos del grupo speech (synthesize / play / list / remove) y del
        # arrastre de cleanup; con default inocuo para el resto de comandos.
        self.label = kwargs.get("label", "etiqueta")
        self.play = kwargs.get("play", False)
        self.force = kwargs.get("force", False)
        self.synthetic_speech = kwargs.get("synthetic_speech", False)
        self.voices = kwargs.get("voices", False)
        self.dry_run = kwargs.get("dry_run", False)
        self.cleanup_parser = kwargs.get("cleanup_parser", None)


class TestResolveVoicePaths:
    def test_resolve_from_voice_name_not_found(self):
        from tts_sidecar.cli import _resolve_voice_paths
        with patch("os.path.exists", return_value=False):
            args = MockArgs(voice="nonexistent")
            with pytest.raises(FileNotFoundError):
                _resolve_voice_paths(args)

    @patch("os.path.exists", return_value=True)
    def test_resolve_from_voice_name_found(self, mock_exists):
        from tts_sidecar.cli import _resolve_voice_paths
        args = MockArgs(voice="crist")
        va, sa = _resolve_voice_paths(args)
        assert va is not None
        assert sa is not None

    @patch("os.path.exists", return_value=True)
    def test_resolve_defaults_to_default_voice_when_no_voice_given(self, mock_exists):
        from tts_sidecar.cli import _resolve_voice_paths
        args = MockArgs()
        va, sa = _resolve_voice_paths(args)
        assert va is not None
        assert sa is not None


class TestCmdVoiceList:
    @patch("tts_sidecar.voices.list_voices")
    def test_cmd_voice_list_lists_voices(self, mock_list_voices, capsys):
        from tts_sidecar.cli import cmd_voice_list

        mock_list_voices.return_value = ["crist", "testcli"]

        cmd_voice_list(MockArgs())

        out = capsys.readouterr().out
        assert "Voces registradas:" in out
        assert "crist" in out
        assert "testcli" in out

    @patch("tts_sidecar.voices.list_voices")
    def test_cmd_voice_list_empty(self, mock_list_voices, capsys):
        from tts_sidecar.cli import cmd_voice_list

        mock_list_voices.return_value = []

        cmd_voice_list(MockArgs())

        out = capsys.readouterr().out
        assert "No hay voces registradas" in out

    @patch("tts_sidecar.voices.list_voices")
    def test_cmd_voice_list_json(self, mock_list_voices, capsys):
        import json
        from tts_sidecar.cli import SCHEMA_VERSION, cmd_voice_list

        mock_list_voices.return_value = ["crist", "testcli"]

        cmd_voice_list(MockArgs(json=True))

        out = capsys.readouterr().out
        assert json.loads(out) == {
            "schema_version": SCHEMA_VERSION, "voices": ["crist", "testcli"],
        }


class TestCmdVoiceClone:
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=False)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_requires_model(self, mock_register, _cached, capsys):
        """Sin modelo cacheado, el clonado aborta (exit 4) antes de copiar audios."""
        from tts_sidecar.cli import cmd_voice_clone, EXIT_MODEL_MISSING

        with pytest.raises(CliError) as exc:
            cmd_voice_clone(MockArgs(name="newvoice", timbre_reference="timbre.wav", speech_reference="speech.wav"))

        assert exc.value.code == EXIT_MODEL_MISSING
        assert "setup" in exc.value.message
        mock_register.assert_not_called()

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_precomputes_via_daemon(
        self, mock_register, _cached, mock_client_cls, _running, capsys
    ):
        """Con daemon activo, precomputa vía IPC sin cargar el motor en frío."""
        from tts_sidecar.cli import cmd_voice_clone

        mock_register.return_value = ("/path/to/timbre-reference.wav", "/path/to/speech-reference.wav")
        mock_client_cls.return_value.precompute_voice.return_value = True

        with patch("tts_sidecar.engine.ChatterboxEngine") as mock_engine_cls:
            cmd_voice_clone(MockArgs(name="newvoice", timbre_reference="timbre.wav", speech_reference="speech.wav"))
            mock_engine_cls.assert_not_called()

        out = capsys.readouterr().out
        assert "Voz 'newvoice' clonada" in out
        assert "precomputados" in out
        mock_client_cls.return_value.precompute_voice.assert_called_once_with("newvoice")

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_precomputes_direct(
        self, mock_register, _cached, _running, capsys
    ):
        """Sin daemon, carga el motor en modo directo y precomputa."""
        from tts_sidecar.cli import cmd_voice_clone

        mock_register.return_value = ("/path/to/timbre-reference.wav", "/path/to/speech-reference.wav")

        with patch("tts_sidecar.engine.ChatterboxEngine") as mock_engine_cls:
            engine = mock_engine_cls.get_instance.return_value
            cmd_voice_clone(MockArgs(name="newvoice", timbre_reference="timbre.wav", speech_reference="speech.wav"))
            engine.precompute_voice.assert_called_once_with("newvoice")

        assert "precomputados" in capsys.readouterr().out

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_precompute_failure_non_fatal(
        self, mock_register, _cached, _running, capsys
    ):
        """Un fallo del precómputo avisa pero no aborta el clonado (lazy fallback)."""
        from tts_sidecar.cli import cmd_voice_clone

        mock_register.return_value = ("/path/to/timbre-reference.wav", "/path/to/speech-reference.wav")

        with patch("tts_sidecar.engine.ChatterboxEngine") as mock_engine_cls:
            mock_engine_cls.get_instance.side_effect = RuntimeError("boom")
            cmd_voice_clone(MockArgs(name="newvoice", timbre_reference="timbre.wav", speech_reference="speech.wav"))

        captured = capsys.readouterr()
        assert "Voz 'newvoice' clonada" in captured.out
        assert "primera síntesis" in captured.out
        assert "Advertencia" in captured.err

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_json_includes_precomputed(
        self, mock_register, _cached, mock_client_cls, _running, capsys
    ):
        """El payload --json incluye la clave precomputed."""
        import json
        from tts_sidecar.cli import cmd_voice_clone, SCHEMA_VERSION

        mock_register.return_value = ("/path/to/timbre-reference.wav", "/path/to/speech-reference.wav")
        mock_client_cls.return_value.precompute_voice.return_value = True

        cmd_voice_clone(MockArgs(name="newvoice", timbre_reference="timbre.wav", speech_reference="speech.wav", json=True))

        assert json.loads(capsys.readouterr().out) == {
            "schema_version": SCHEMA_VERSION,
            "name": "newvoice",
            "timbre": "/path/to/timbre-reference.wav",
            "speech": "/path/to/speech-reference.wav",
            "precomputed": True,
        }

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_cmd_voice_clone_text_output_without_timbre_has_no_none(
        self, mock_register, _cached, _running, capsys
    ):
        """Sin timbre, la salida de texto no debe contener la subcadena 'None'."""
        from tts_sidecar.cli import cmd_voice_clone

        mock_register.return_value = (None, "/path/to/speech-reference.wav")

        with patch("tts_sidecar.engine.ChatterboxEngine") as mock_engine_cls:
            cmd_voice_clone(MockArgs(name="newvoice", timbre_reference=None, speech_reference="speech.wav"))
            mock_engine_cls.get_instance.return_value.precompute_voice.assert_called_once_with("newvoice")

        out = capsys.readouterr().out
        assert "None" not in out


class TestCmdVoiceRemove:
    @patch("tts_sidecar.voices.remove_voice")
    def test_cmd_voice_remove_success(self, mock_remove_voice, capsys):
        from tts_sidecar.cli import cmd_voice_remove

        mock_remove_voice.return_value = True

        cmd_voice_remove(MockArgs(name="testcli"))

        out = capsys.readouterr().out
        assert "Voz 'testcli' eliminada" in out

    @patch("tts_sidecar.voices.remove_voice")
    def test_cmd_voice_remove_not_found(self, mock_remove_voice, capsys):
        from tts_sidecar.cli import cmd_voice_remove

        mock_remove_voice.return_value = False

        with pytest.raises(CliError):
            cmd_voice_remove(MockArgs(name="nonexistent"))


class TestVoiceMessages:
    @patch("tts_sidecar.voices._resolve_voice_dir")
    @patch("tts_sidecar.voices.remove_voice", return_value=False)
    def test_remove_of_factory_voice_explains_read_only(
        self, mock_remove, mock_resolve, capsys
    ):
        from tts_sidecar.cli import cmd_voice_remove

        mock_resolve.return_value = "/fabrica/default"

        with pytest.raises(CliError):
            cmd_voice_remove(MockArgs(name="default"))

        err = capsys.readouterr().err
        assert "voz de fábrica" in err
        assert "no encontrada" not in err

    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    def test_speech_say_does_not_refer_to_setup_if_user_audio_missing(self, _cached):
        from tts_sidecar.cli import cmd_speech_say
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        # Voz inexistente con el modelo en caché: sale 3 (recurso ausente) y su
        # mensaje no remite a 'setup' (descargar el modelo no crea la voz).
        with pytest.raises(CliError) as exc:
            cmd_speech_say(MockArgs(text="hola", voice="voz_inexistente", no_daemon=True))

        assert exc.value.code == EXIT_NOT_FOUND
        assert "Error:" in exc.value.message
        assert "setup" not in exc.value.message


class TestCmdDevices:
    @patch("tts_sidecar.audio.get_audio_devices")
    def test_cmd_devices(self, mock_get_devices, capsys):
        from tts_sidecar.cli import cmd_devices

        mock_get_devices.return_value = [
            {"id": 0, "name": "Speaker 1", "latency": 0.01},
            {"id": 1, "name": "Speaker 2", "latency": 0.005},
        ]

        cmd_devices(MockArgs())

        out = capsys.readouterr().out
        assert "Dispositivos de salida de audio:" in out
        assert "Speaker 1" in out
        assert "Speaker 2" in out

    @patch("tts_sidecar.audio.get_audio_devices")
    def test_cmd_devices_json(self, mock_get_devices, capsys):
        import json
        from tts_sidecar.cli import SCHEMA_VERSION, cmd_devices

        devices = [{"id": 0, "name": "Speaker 1", "latency": 0.01}]
        mock_get_devices.return_value = devices

        cmd_devices(MockArgs(json=True))

        out = capsys.readouterr().out
        assert json.loads(out) == {
            "schema_version": SCHEMA_VERSION, "devices": devices,
        }


class TestCmdVersion:
    def test_cmd_version_human(self, capsys):
        from tts_sidecar.cli import cmd_version

        cmd_version(MockArgs())

        out = capsys.readouterr().out
        assert "tts-sidecar" in out

    def test_cmd_version_json(self, capsys):
        import json
        from tts_sidecar import __version__
        from tts_sidecar.cli import SCHEMA_VERSION, cmd_version

        cmd_version(MockArgs(json=True))

        out = capsys.readouterr().out
        assert json.loads(out) == {
            "schema_version": SCHEMA_VERSION,
            "name": "tts-sidecar", "version": __version__,
        }


class TestCmdSpeechSayDaemonDispatch:
    """Las tres ramas del despacho daemon/auto/directo."""

    def _args(self, **kw):
        kw.setdefault("timbre_reference", _make_wav("v.wav"))
        kw.setdefault("speech_reference", _make_wav("s.wav"))
        return MockArgs(**kw)

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_without_flags_uses_daemon_if_responsive(self, mock_running, mock_client_cls, _cached, mock_player_cls):
        from tts_sidecar.cli import cmd_speech_say

        client = MagicMock()
        client.synthesize.return_value = _synth_result()
        mock_client_cls.return_value = client

        cmd_speech_say(self._args())

        mock_running.assert_called_once()
        client.synthesize.assert_called_once()

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    def test_without_flags_falls_back_to_direct_if_unresponsive(self, mock_running, mock_engine_cls, _cached, mock_player_cls):
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args())

        mock_running.assert_called_once()
        engine.synthesize.assert_called_once()

    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    def test_explicit_daemon_requires_running_and_exits_5(self, mock_running, mock_client_cls, _cached):
        from tts_sidecar.cli import cmd_speech_say
        from tts_sidecar.exit_codes import EXIT_DAEMON_UNREACHABLE

        # --daemon EXIGE el daemon (§2.5): si el sondeo dice que no está activo,
        # sale con 5 sin intentar sintetizar.
        with pytest.raises(CliError) as exc:
            cmd_speech_say(self._args(daemon=True))

        assert exc.value.code == EXIT_DAEMON_UNREACHABLE
        mock_running.assert_called_once()
        mock_client_cls.return_value.synthesize.assert_not_called()

    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_explicit_daemon_running_but_ipc_fails_exits_5(self, mock_running, mock_client_cls, _cached):
        from tts_sidecar.cli import cmd_speech_say
        from tts_sidecar.daemon import DaemonIPCError
        from tts_sidecar.exit_codes import EXIT_DAEMON_UNREACHABLE

        # Con el daemon activo pero la IPC fallando, sigue siendo inalcanzabilidad: 5.
        client = MagicMock()
        client.synthesize.side_effect = DaemonIPCError("no conecta")
        mock_client_cls.return_value = client

        with pytest.raises(CliError) as exc:
            cmd_speech_say(self._args(daemon=True))

        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    @patch("tts_sidecar.daemon.is_daemon_running")
    def test_no_daemon_does_not_probe(self, mock_running, mock_engine_cls, _cached, mock_player_cls):
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(no_daemon=True))

        mock_running.assert_not_called()
        engine.synthesize.assert_called_once()


class TestCmdSpeechLiveProgress:
    """El progreso se cablea desde la fuente de eventos hasta el Spinner en
    ambos modos: on_progress (daemon) y progress_callback (directo)."""

    def _args(self, **kw):
        kw.setdefault("timbre_reference", _make_wav("v.wav"))
        kw.setdefault("speech_reference", _make_wav("s.wav"))
        return MockArgs(**kw)

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_daemon_passes_formatted_on_progress(
        self, mock_running, mock_client_cls, _cached, mock_player_cls
    ):
        from tts_sidecar.cli import cmd_speech_say

        client = MagicMock()
        client.synthesize.return_value = _synth_result()
        mock_client_cls.return_value = client

        cmd_speech_say(self._args(daemon=True))

        _, kwargs = client.synthesize.call_args
        on_progress = kwargs.get("on_progress")
        assert callable(on_progress), "el daemon debe recibir un on_progress cableado"
        # El callback formatea el evento y actualiza el spinner sin lanzar
        # (en no-TTY el spinner es un no-op, pero la ruta debe ser segura).
        on_progress({"event": "progress", "stage": "t3", "tokens": 42})

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=False)
    def test_direct_passes_formatted_progress_callback(
        self, mock_running, mock_engine_cls, _cached, mock_player_cls
    ):
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(no_daemon=True))

        _, kwargs = engine.synthesize.call_args
        progress_callback = kwargs.get("progress_callback")
        assert callable(progress_callback), "el modo directo debe cablear progress_callback"
        progress_callback({"event": "progress", "stage": "s3gen"})


class TestCmdSpeech:
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_cmd_speech_plays_without_output(self, mock_engine_cls, mock_player_cls, mock_cached):
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine
        player = MagicMock()
        mock_player_cls.return_value = player

        cmd_speech_say(MockArgs(text="hola", no_daemon=True))

        player.play.assert_called_once_with(b"RIFF")


class TestEnvironmentChecksAudio:
    """El chequeo de audio de doctor/setup refleja el estado real
    de la enumeración COM en Windows, no solo la disponibilidad del import."""

    def test_windows_real_audio_gives_pass(self, monkeypatch):
        import platform as platform_module
        from tts_sidecar import cli

        monkeypatch.setattr(platform_module, "system", lambda: "Windows")
        monkeypatch.setattr(
            "tts_sidecar.audio.get_audio_devices_with_status",
            lambda: ([{"id": 0, "name": "Altavoices"}], False),
        )

        checks = cli._environment_checks()
        audio_check = next(c for c in checks if c[1] == "Audio library")
        assert audio_check[0] == "PASS"
        assert "1 dispositivo" in audio_check[2]

    def test_windows_degraded_audio_gives_fail(self, monkeypatch):
        import platform as platform_module
        from tts_sidecar import cli

        monkeypatch.setattr(platform_module, "system", lambda: "Windows")
        monkeypatch.setattr(
            "tts_sidecar.audio.get_audio_devices_with_status",
            lambda: ([{"id": 0, "name": "Default", "latency": 0.1}], True),
        )

        checks = cli._environment_checks()
        audio_check = next(c for c in checks if c[1] == "Audio library")
        assert audio_check[0] == "FAIL"
        assert "no se pudo enumerar" in audio_check[2]

    def test_linux_degraded_audio_gives_fail(self, monkeypatch):
        """En Linux el detalle degradado apunta a la causa PortAudio/libportaudio2."""
        import platform as platform_module
        from tts_sidecar import cli

        monkeypatch.setattr(platform_module, "system", lambda: "Linux")
        monkeypatch.setattr(
            "tts_sidecar.audio.get_audio_devices_with_status",
            lambda: ([{"id": 0, "name": "Default", "latency": 0.1}], True),
        )

        checks = cli._environment_checks()
        audio_check = next(c for c in checks if c[1] == "Audio library")
        assert audio_check[0] == "FAIL"
        assert "PortAudio" in audio_check[2]
        assert "libportaudio2" in audio_check[2]

    def test_macos_real_audio_gives_pass(self, monkeypatch):
        import platform as platform_module
        from tts_sidecar import cli

        monkeypatch.setattr(platform_module, "system", lambda: "Darwin")
        monkeypatch.setattr(
            "tts_sidecar.audio.get_audio_devices_with_status",
            lambda: ([{"id": 0, "name": "Built-in Output"}], False),
        )

        checks = cli._environment_checks()
        audio_check = next(c for c in checks if c[1] == "Audio library")
        assert audio_check[0] == "PASS"
        assert "1 dispositivo" in audio_check[2]


class TestPlayAudioMissingPortAudio:
    def test_oserror_from_player_becomes_precondition_cli_error(self):
        """Si AudioPlayer falla por PortAudio ausente, _play_audio lo traduce a
        CliError(EXIT_PRECONDITION_FAILED) en vez de propagar un OSError crudo."""
        from tts_sidecar import cli
        from tts_sidecar.exit_codes import EXIT_PRECONDITION_FAILED

        with patch(
            "tts_sidecar.audio.AudioPlayer",
            side_effect=OSError("falta libportaudio2"),
        ):
            with pytest.raises(CliError) as exc:
                cli._play_audio(b"RIFF....WAVEfmt ")

        assert exc.value.code == EXIT_PRECONDITION_FAILED
        assert exc.value.reason == "audio_library_missing"
        assert "libportaudio2" in exc.value.message


class TestCmdDevicesError:
    @patch("tts_sidecar.audio.get_audio_devices")
    def test_cmd_devices_exception_exits_code_1(self, mock_get_devices, capsys):
        from tts_sidecar.cli import cmd_devices

        mock_get_devices.side_effect = RuntimeError("PortAudio no disponible")

        with pytest.raises(CliError) as exc:
            cmd_devices(MockArgs())

        assert "Error" in exc.value.message


# En Windows, crear symlinks sin privilegios elevados exige Developer
# Mode (SeCreateSymbolicLinkPrivilege) habilitado; en CI/runners sin esa
# configuración, os.symlink levanta OSError (WinError 1314). En Linux/macOS
# los symlinks de usuario funcionan sin configuración especial, así que el
# skip real solo ocurre en Windows sin Developer Mode. La razón del skip es
# explícita y accionable (a diferencia de un return silencioso) para que un
# run local en un Windows sin Developer Mode explique por qué faltan estos
# tests en vez de aparentar cobertura completa.
_SYMLINK_SKIP_REASON = (
    "el entorno no permite crear symlinks (en Windows: habilita Developer "
    "Mode en Configuración > Privacidad y seguridad > Para programadores, o "
    "corre con privilegios elevados)"
)


def _symlinks_supported(tmp_path) -> bool:
    """Sondea si el proceso actual puede crear symlinks en `tmp_path`.

    En Windows depende de Developer Mode o de privilegios elevados; en
    Linux/macOS los symlinks de usuario no requieren configuración especial,
    así que esto normalmente solo es False en Windows sin Developer Mode.
    """
    probe = tmp_path / "_symlink_probe"
    try:
        probe.symlink_to(tmp_path)
        probe.unlink()
        return True
    except OSError:
        return False


class TestSetupLinuxPath:
    """Integración de PATH de setup en Linux (symlink $APPIMAGE → ~/.local/bin)."""

    def _fake_home(self, monkeypatch, tmp_path):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
        return home

    def _linux_appimage_env(self, monkeypatch, tmp_path):
        appimage = tmp_path / "tts-sidecar-x86_64.AppImage"
        appimage.write_bytes(b"fake appimage")
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setenv("APPIMAGE", str(appimage))
        return appimage

    def test_creates_symlink_from_appimage(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        appimage = self._linux_appimage_env(monkeypatch, tmp_path)

        _integrate_linux_path()

        link = home / ".local" / "bin" / "tts-sidecar"
        assert link.is_symlink()
        assert link.resolve() == appimage.resolve()
        assert "symlink creado" in capsys.readouterr().err

    def test_creates_symlink_from_externally_exported_appimage(self, monkeypatch, tmp_path, capsys):
        # Contrato oficial: install-linux.sh exporta APPIMAGE tras instalar el AppImage
        # en ~/.local/opt/tts-sidecar/, sin correr dentro de un runtime AppImage
        # real. El symlink debe crearse igual que si lo exportara el runtime.
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        install_dir = tmp_path / "opt" / "tts-sidecar"
        install_dir.mkdir(parents=True)
        appimage = install_dir / "tts-sidecar-x86_64.AppImage"
        appimage.write_bytes(b"appimage instalado por install-linux.sh")
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setenv("APPIMAGE", str(appimage))

        _integrate_linux_path()

        link = home / ".local" / "bin" / "tts-sidecar"
        assert link.is_symlink()
        assert link.resolve() == appimage.resolve()

    def test_appimage_pointing_to_missing_file_is_skipped(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setenv("APPIMAGE", str(tmp_path / "no-existe.AppImage"))

        _integrate_linux_path()

        assert not (home / ".local").exists()
        assert "no apunta a un archivo existente" in capsys.readouterr().err

    def test_updates_existing_symlink_idempotent(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        appimage = self._linux_appimage_env(monkeypatch, tmp_path)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path / "otro-viejo.AppImage")

        _integrate_linux_path()
        _integrate_linux_path()  # segunda pasada: idempotente

        assert link.is_symlink()
        assert link.resolve() == appimage.resolve()

    def test_without_appimage_does_not_touch_filesystem(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.delenv("APPIMAGE", raising=False)

        _integrate_linux_path()

        assert not (home / ".local").exists()

    def test_does_not_overwrite_regular_file(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import _integrate_linux_path

        home = self._fake_home(monkeypatch, tmp_path)
        self._linux_appimage_env(monkeypatch, tmp_path)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.write_text("no soy un symlink", encoding="utf-8")

        _integrate_linux_path()

        assert not link.is_symlink()
        assert link.read_text(encoding="utf-8") == "no soy un symlink"
        assert "no se modifica" in capsys.readouterr().err

    def test_remove_path_elimina_symlink(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home(monkeypatch, tmp_path)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)

        cmd_setup(MockArgs(remove_path=True))

        assert not link.exists()
        assert "Symlink eliminado" in capsys.readouterr().err

    def test_remove_path_sin_symlink_informa(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup

        self._fake_home(monkeypatch, tmp_path)

        cmd_setup(MockArgs(remove_path=True))

        assert "No hay nada que quitar" in capsys.readouterr().err

    def test_remove_path_rechaza_archivo_regular(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home(monkeypatch, tmp_path)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.write_text("no soy un symlink", encoding="utf-8")

        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(remove_path=True))

        assert link.exists()
        assert "no es un symlink" in exc.value.message

    def test_path_warning_uses_posix_paths(self, monkeypatch, tmp_path, capsys):
        # L-01: la línea sugerida debe ser bash válido (forward slashes),
        # nunca rutas con backslashes que romperían el shell profile.
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import _integrate_linux_path

        self._fake_home(monkeypatch, tmp_path)
        self._linux_appimage_env(monkeypatch, tmp_path)
        # Garantiza que ~/.local/bin no esté en el PATH de la sesión.
        monkeypatch.setenv("PATH", "/usr/bin")

        _integrate_linux_path()

        out = capsys.readouterr().err
        assert 'export PATH="$HOME/.local/bin:$PATH"' in out
        assert "~/.bashrc, ~/.zshrc" in out
        # La línea sugerida y los profiles nunca deben llevar backslashes
        # (las rutas absolutas del symlink sí pueden, si el test corre en Windows).
        assert "$HOME\\.local" not in out and "~\\.bashrc" not in out

    def test_setup_integrates_path_before_failed_checks(self, monkeypatch, tmp_path, capsys):
        # L-02: un host degradado (chequeo FAIL) debe obtener igualmente el
        # comando en el PATH, en paridad con Windows y macOS. Se usa un FAIL
        # no-audio porque el de audio ya no aborta setup (A-01).
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        import tts_sidecar.cli as cli

        home = self._fake_home(monkeypatch, tmp_path)
        appimage = self._linux_appimage_env(monkeypatch, tmp_path)
        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("FAIL", "Chatterbox TTS", "NO INSTALADO")],
        )

        with pytest.raises(CliError):
            cli.cmd_setup(MockArgs(remove_path=False))

        link = home / ".local" / "bin" / "tts-sidecar"
        assert link.is_symlink()
        assert link.resolve() == appimage.resolve()


class TestSetupAudioAdvisory:
    """A-01: setup es provisión, no diagnóstico — el FAIL de audio se degrada
    a WARN y la provisión continúa; doctor conserva el FAIL con salida 1."""

    def test_audio_fail_does_not_abort_setup_and_reaches_provisioning(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7"),
                     ("FAIL", "Audio library", "sin subsistema de sonido")],
        )
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True):
            cli.cmd_setup(MockArgs(remove_path=False))  # no debe lanzar SystemExit

        out = capsys.readouterr().err
        assert "[WARN] Audio library" in out
        assert "speech synthesize" in out
        assert "Provisión completa" in out

    def test_non_audio_fail_still_aborts_setup(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("FAIL", "Chatterbox TTS", "NO INSTALADO")],
        )
        with pytest.raises(CliError) as exc:
            cli.cmd_setup(MockArgs(remove_path=False))

        assert "[FAIL] Chatterbox TTS" in exc.value.message

    def test_doctor_keeps_audio_fail_with_exit_1(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("FAIL", "Audio library", "sin subsistema de sonido")],
        )
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True):
            # Salida por veredicto: retorna EXIT_ERROR en vez de levantar.
            result = cli.cmd_doctor(MockArgs(json=False))

        assert result == cli.EXIT_ERROR
        captured = capsys.readouterr()
        assert "[FAIL] Audio library" in captured.out
        # La ruta humana no duplica la línea de resumen en stderr.
        assert captured.err == ""


class TestCheckAvx2:
    """Chequeo best-effort de AVX2, por arquitectura y SO. Nunca FAIL:
    PASS/WARN donde hay detección (Linux, macOS Intel) y SKIP informativo donde
    no la hay (Windows, ARM)."""

    def test_non_x86_reports_not_applicable(self, monkeypatch):
        import platform as platform_mod
        import tts_sidecar.cli as cli

        monkeypatch.setattr(platform_mod, "machine", lambda: "arm64")
        status, name, detail = cli._check_avx2()
        assert (status, name) == ("SKIP", "CPU AVX2")
        assert "no aplica" in detail

    def test_windows_degrades_to_informative_skip(self, monkeypatch):
        import platform as platform_mod
        import tts_sidecar.cli as cli

        monkeypatch.setattr(platform_mod, "machine", lambda: "AMD64")
        monkeypatch.setattr(sys, "platform", "win32")
        status, name, detail = cli._check_avx2()
        assert status == "SKIP"
        assert "Windows" in detail

    def _fake_cpuinfo(self, monkeypatch, tmp_path, flags_line):
        import tts_sidecar.cli as cli
        from pathlib import Path as RealPath

        fake = tmp_path / "cpuinfo"
        fake.write_text(flags_line, encoding="utf-8")
        monkeypatch.setattr(
            cli, "Path",
            lambda p="": RealPath(fake) if str(p) == "/proc/cpuinfo" else RealPath(p),
        )

    def test_linux_with_avx2_flag_passes(self, monkeypatch, tmp_path):
        import platform as platform_mod
        import tts_sidecar.cli as cli

        monkeypatch.setattr(platform_mod, "machine", lambda: "x86_64")
        monkeypatch.setattr(sys, "platform", "linux")
        self._fake_cpuinfo(monkeypatch, tmp_path, "flags\t\t: fpu avx avx2 sse4_2\n")
        assert cli._check_avx2()[0] == "PASS"

    def test_linux_without_avx2_flag_warns(self, monkeypatch, tmp_path):
        import platform as platform_mod
        import tts_sidecar.cli as cli

        monkeypatch.setattr(platform_mod, "machine", lambda: "x86_64")
        monkeypatch.setattr(sys, "platform", "linux")
        self._fake_cpuinfo(monkeypatch, tmp_path, "flags\t\t: fpu avx sse4_2\n")
        status, _, detail = cli._check_avx2()
        assert status == "WARN"
        assert "PyTorch" in detail


class TestCheckOnedrive:
    """Chequeo informativo (WARN) de data_root() bajo OneDrive en Windows.
    Fuera de Windows es SKIP; en Windows es PASS salvo que data_root() caiga bajo
    la sincronización de OneDrive (WARN). Nunca FAIL: no altera el exit code."""

    def _patch(self, monkeypatch, data_root_path, platform="win32",
               onedrive=None, onedrive_commercial=None):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(sys, "platform", platform)
        monkeypatch.setattr(
            "tts_sidecar.paths.data_root", lambda: data_root_path)
        # Aísla el caso limpiando las variables de entorno de OneDrive.
        monkeypatch.delenv("OneDrive", raising=False)
        monkeypatch.delenv("OneDriveCommercial", raising=False)
        if onedrive is not None:
            monkeypatch.setenv("OneDrive", onedrive)
        if onedrive_commercial is not None:
            monkeypatch.setenv("OneDriveCommercial", onedrive_commercial)
        return cli

    def test_non_windows_is_skip(self, monkeypatch):
        cli = self._patch(monkeypatch, "x", platform="darwin")
        status, name, detail = cli._check_onedrive()
        assert (status, name) == ("SKIP", "OneDrive user-data-dir")
        assert "fuera de Windows" in detail

    def test_windows_under_onedrive_env_var_warns(self, monkeypatch):
        cli = self._patch(
            monkeypatch, r"C:\Users\test\OneDrive\tts-sidecar",
            onedrive=r"C:\Users\test\OneDrive")
        status, name, detail = cli._check_onedrive()
        assert status == "WARN"
        assert "OneDrive" in detail
        assert "Files On-Demand" in detail

    def test_windows_under_onedrive_path_pattern_warns(self, monkeypatch):
        cli = self._patch(
            monkeypatch, r"C:\Users\test\OneDrive - Company\tts-sidecar")
        status, name, detail = cli._check_onedrive()
        assert status == "WARN"
        assert "Files On-Demand" in detail

    def test_windows_normal_passes(self, monkeypatch):
        cli = self._patch(
            monkeypatch, r"C:\Users\test\AppData\Local\tts-sidecar")
        status, name, detail = cli._check_onedrive()
        assert (status, name) == ("PASS", "OneDrive user-data-dir")
        assert "no detectado" in detail


class TestInterruptHandling:
    """Ctrl+C termina con código 130 y una línea a stderr, sin traceback."""

    def test_ctrl_c_exits_130_without_traceback(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        def _interrumpe(args):
            raise KeyboardInterrupt

        monkeypatch.setattr(sys, "argv", ["tts-sidecar", "version"])
        monkeypatch.setattr(cli, "cmd_version", _interrumpe)

        with pytest.raises(SystemExit) as exc_info:
            cli.main()

        assert exc_info.value.code == 130
        captured = capsys.readouterr()
        assert "Interrumpido por el usuario." in captured.err
        assert "Traceback" not in captured.err
        assert "KeyboardInterrupt" not in captured.err


class TestExitCodes:
    """Cada causa de error mapea a su código del contrato público congelado."""

    def test_missing_model_exits_4(self, capsys):
        from tts_sidecar.cli import _require_model_cached, EXIT_MODEL_MISSING

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(CliError) as exc:
                _require_model_cached()
        assert exc.value.code == EXIT_MODEL_MISSING
        assert "setup" in exc.value.message

    def test_empty_text_exits_2(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT

        with pytest.raises(CliError) as exc:
            cmd_speech_say(MockArgs(text="   ", no_daemon=True))
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_say_model_missing_exits_4(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_MODEL_MISSING

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(CliError) as exc:
                cmd_speech_say(MockArgs(text="hola", no_daemon=True))
        assert exc.value.code == EXIT_MODEL_MISSING

    def test_nonexistent_voice_exits_3(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_NOT_FOUND

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True):
            with pytest.raises(CliError) as exc:
                cmd_speech_say(MockArgs(text="hola", voice="voz_inexistente", no_daemon=True))
        assert exc.value.code == EXIT_NOT_FOUND

    def test_unreachable_daemon_with_flag_exits_5(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_DAEMON_UNREACHABLE
        from tts_sidecar.daemon import DaemonIPCError

        def _falla(args, voice):
            raise DaemonIPCError("no se puede conectar al daemon")

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("tts_sidecar.cli._synthesize_via_daemon", side_effect=_falla):
            with pytest.raises(CliError) as exc:
                cmd_speech_say(MockArgs(
                    text="hola",
                    daemon=True,
                ))
        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    def test_generic_error_exits_1(self):
        from tts_sidecar.cli import cmd_devices, EXIT_ERROR

        with patch("tts_sidecar.audio.get_audio_devices", side_effect=RuntimeError("boom")):
            with pytest.raises(CliError) as exc:
                cmd_devices(MockArgs())
        assert exc.value.code == EXIT_ERROR

    def test_voice_clone_collision_exits_6(self):
        """Colisión de nombre sin --force → EXIT_STATE_CONFLICT (6): el recurso
        está ocupado, distinto del EXIT_INVALID_INPUT (2) del audio ilegible."""
        from tts_sidecar.cli import cmd_voice_clone, EXIT_STATE_CONFLICT
        from tts_sidecar.voices import VoiceExistsError

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("tts_sidecar.voices.clone_voice_files",
                      side_effect=VoiceExistsError("La voz 'dup' ya existe")):
            with pytest.raises(CliError) as exc:
                cmd_voice_clone(MockArgs(name="dup"))
        assert exc.value.code == EXIT_STATE_CONFLICT

    def test_voice_clone_unreadable_audio_exits_2(self):
        """Audio ilegible → EXIT_INVALID_INPUT (2): ValueError genérico, no
        colisión; el 2 y el 6 no se colapsan en un solo entero."""
        from tts_sidecar.cli import cmd_voice_clone, EXIT_INVALID_INPUT

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("tts_sidecar.voices.clone_voice_files",
                      side_effect=ValueError("El audio de speech no es cargable")):
            with pytest.raises(CliError) as exc:
                cmd_voice_clone(MockArgs(name="dup"))
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_daemon_and_no_daemon_conflict_exits_2(self, monkeypatch, capsys):
        """--daemon y --no-daemon simultáneos → argparse los rechaza por el
        grupo mutuamente excluyente, antes de despachar (SystemExit 2)."""
        from tts_sidecar.cli import main

        monkeypatch.setattr(sys, "argv", [
            "tts-sidecar", "speech", "say", "--text", "hola", "--daemon", "--no-daemon",
        ])
        with pytest.raises(SystemExit) as exc:
            main()
        assert exc.value.code == 2
        assert "not allowed with" in capsys.readouterr().err

    def test_voice_list_filenotfound_points_to_voices_dir_not_setup(self, capsys):
        """El FileNotFoundError de voice list menciona el directorio de
        voices, no remite a 'setup' (la provisión del modelo no lo arregla)."""
        from tts_sidecar.cli import cmd_voice_list, EXIT_NOT_FOUND

        with patch("tts_sidecar.voices.list_voices",
                   side_effect=FileNotFoundError("directorio ilegible")), \
                patch("tts_sidecar.voices.voices_root", return_value="/ruta/voices"):
            with pytest.raises(CliError) as exc:
                cmd_voice_list(MockArgs())
        assert exc.value.code == EXIT_NOT_FOUND
        assert "/ruta/voices" in exc.value.message
        assert "setup" not in exc.value.message

    def test_daemon_start_failure_exits_5(self):
        import argparse
        from tts_sidecar.cli import cmd_daemon, EXIT_DAEMON_UNREACHABLE

        args = argparse.Namespace(action="start", autorestart=False, max_retries=0, port=None)
        manager = MagicMock()
        manager.start.return_value = False

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("tts_sidecar.daemon.DaemonManager", return_value=manager):
            with pytest.raises(CliError) as exc:
                cmd_daemon(args)
        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    def test_daemon_serve_without_model_exits_and_skips_serve(self):
        """'daemon serve' sin modelo en caché falla rápido remitiendo a
        'setup' (exit EXIT_MODEL_MISSING) y NO carga/arranca el servidor."""
        import argparse
        from tts_sidecar.cli import cmd_daemon, EXIT_MODEL_MISSING

        args = argparse.Namespace(action="serve", auto_restart=False, max_retries=0)
        serve = MagicMock()

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("tts_sidecar.daemon.run.serve", serve):
            with pytest.raises(CliError) as exc:
                cmd_daemon(args)
        assert exc.value.code == EXIT_MODEL_MISSING
        serve.assert_not_called()


class TestSpeechLanguageCrossLingual:
    """`--language` y los overrides de síntesis en `speech say`/`speech synthesize`
    (Fase 3 del rediseño cross-lingual, §3.4/§3.5/§3.12 de cli-redesign.md)."""

    def _args(self, **kw):
        kw.setdefault("timbre_reference", _make_wav("v.wav"))
        kw.setdefault("speech_reference", _make_wav("s.wav"))
        kw.setdefault("no_daemon", True)
        return MockArgs(**kw)

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_say_language_en_dispatches_english_model(self, mock_engine_cls, _cached, _player):
        """--language en resuelve get_instance(model="en", ...) y no revienta:
        detrás del engine.synthesize real (no probado aquí) queda la ramificación
        por idioma ya cubierta en test_synthesis_orchestrator.py."""
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(text="hello", language="en"))

        _, kwargs = mock_engine_cls.get_instance.call_args
        assert kwargs["model"] == "en"

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_say_language_es_latam_unchanged(self, mock_engine_cls, _cached, _player):
        """Sin --language (o con es-latam explícito) el modelo resuelto sigue
        siendo es-mx-latam: retrocompatible con el comportamiento actual."""
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(text="hola", language="es-latam"))

        _, kwargs = mock_engine_cls.get_instance.call_args
        assert kwargs["model"] == "es-mx-latam"

    def test_cfg_weight_zero_exits_2(self):
        """--cfg-weight 0 es el crash conocido del inglés base: se rechaza
        client-side con exit 2, antes de tocar el motor."""
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT

        with pytest.raises(CliError) as exc:
            cmd_speech_say(self._args(text="hola", cfg_weight=0.0))
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_negative_exaggeration_exits_2(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT

        with pytest.raises(CliError) as exc:
            cmd_speech_say(self._args(text="hola", exaggeration=-0.1))
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_non_positive_temperature_exits_2(self):
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT

        with pytest.raises(CliError) as exc:
            cmd_speech_say(self._args(text="hola", temperature=0.0))
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_missing_language_model_exits_4_points_to_setup_language(self):
        """Idioma sin modelo instalado sale 4 remitiendo a 'setup --language <x>'."""
        from tts_sidecar.cli import cmd_speech_say, EXIT_MODEL_MISSING

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(CliError) as exc:
                cmd_speech_say(self._args(text="hello", language="en"))
        assert exc.value.code == EXIT_MODEL_MISSING
        assert "setup --language en" in exc.value.message

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_overrides_threaded_to_engine_synthesize(self, mock_engine_cls, _cached, _player):
        """exaggeration/cfg_weight/temperature llegan intactos a engine.synthesize."""
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(
            text="hola", exaggeration=0.9, cfg_weight=0.2, temperature=0.6,
        ))

        _, kwargs = engine.synthesize.call_args
        assert kwargs["exaggeration"] == 0.9
        assert kwargs["cfg_weight"] == 0.2
        assert kwargs["temperature"] == 0.6


class TestCmdCleanup:
    """El comando cleanup borra solo las rutas del proyecto, con confirmación."""

    def _args(self, **kw):
        import argparse
        ns = argparse.Namespace(
            model=kw.get("model", False),
            voices=kw.get("voices", False),
            synthetic_speech=kw.get("synthetic_speech", False),
            all=kw.get("all", False),
            dry_run=kw.get("dry_run", False),
            yes=kw.get("yes", False),
            json=kw.get("json", False),
            cleanup_parser=MagicMock(),
        )
        return ns

    def _fake_env(self, tmp_path, monkeypatch):
        """Caché HF sintética con las dos carpetas del proyecto, una ajena,
        y un directorio de voices de usuario."""
        hub = tmp_path / "hub"
        propio1 = hub / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        propio2 = hub / "models--ResembleAI--chatterbox"
        ajeno = hub / "models--otro--proyecto"
        for d in (propio1, propio2, ajeno):
            d.mkdir(parents=True)
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))

        voices = tmp_path / "voices"
        (voices / "mi_voz").mkdir(parents=True)
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(voices))

        # Aísla el almacén de habla sintética a tmp_path para que el arrastre de
        # --voices y --synthetic-speech nunca toquen los datos reales del usuario.
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        return propio1, propio2, ajeno, voices

    def test_dry_run_lists_without_deleting(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)

        cmd_cleanup(self._args(all=True, dry_run=True))

        out = capsys.readouterr().out
        assert "dry-run" in out
        assert str(propio1) in out and str(propio2) in out and str(voices) in out
        assert propio1.exists() and propio2.exists() and voices.exists()

    def test_selective_model_deletion_with_confirmation(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)
        monkeypatch.setattr("builtins.input", lambda _: "s")

        cmd_cleanup(self._args(model=True))

        assert not propio1.exists() and not propio2.exists()
        assert ajeno.exists(), "cleanup nunca toca carpetas ajenas de la caché HF"
        assert voices.exists(), "--model no borra las voices de usuario"

    def test_deleting_voices_does_not_touch_model(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)
        monkeypatch.setattr("builtins.input", lambda _: "s")

        cmd_cleanup(self._args(voices=True))

        assert not voices.exists()
        assert propio1.exists() and propio2.exists() and ajeno.exists()

    def test_negative_confirmation_does_not_delete(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)
        monkeypatch.setattr("builtins.input", lambda _: "n")

        cmd_cleanup(self._args(all=True))

        assert "Cancelado" in capsys.readouterr().out
        assert propio1.exists() and propio2.exists() and voices.exists()

    def test_yes_deletes_without_asking_confirmation(self, tmp_path, monkeypatch, capsys):
        """--yes omite input(); útil para invocación programática."""
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)

        def _no_deberia_llamarse(_):
            raise AssertionError("input() no debe llamarse con --yes")
        monkeypatch.setattr("builtins.input", _no_deberia_llamarse)

        cmd_cleanup(self._args(all=True, yes=True))

        assert not propio1.exists() and not propio2.exists() and not voices.exists()
        assert ajeno.exists()

    def test_eof_en_confirmacion_cancela_limpiamente(self, tmp_path, monkeypatch, capsys):
        """stdin cerrado (subprocess sin --yes) no debe producir traceback."""
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)

        def _eof(_):
            raise EOFError()
        monkeypatch.setattr("builtins.input", _eof)

        cmd_cleanup(self._args(all=True))

        assert "Cancelado" in capsys.readouterr().out
        assert propio1.exists() and propio2.exists() and voices.exists()

    def test_without_flags_shows_help_and_does_not_delete(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, ajeno, voices = self._fake_env(tmp_path, monkeypatch)
        args = self._args()

        cmd_cleanup(args)

        args.cleanup_parser.print_help.assert_called_once()
        assert propio1.exists() and voices.exists()


class TestSetupUninstall:
    """setup --uninstall: desinstalación de un comando en los 3 SO (dispatch por
    SO sobre el contrato compartido). Ver docs/ROADMAP.md §Plan técnico."""

    # ---- Fixtures compartidos ------------------------------------------------

    def _fake_home_linux(self, monkeypatch, tmp_path):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
        monkeypatch.setattr(sys, "platform", "linux")
        # El guard de canal nativo exige modo congelado en las tres ramas.
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        return home

    def _fake_cleanup_env(self, tmp_path, monkeypatch, voices_inside_root=True):
        """Caché HF sintética + voices de usuario + data_root mockeado.

        data_root se mockea a un directorio bajo tmp_path para que el borrado del
        directorio raíz vacío del contrato compartido no toque el HOME real. Con
        voices_inside_root las voices cuelgan de data_root (como en producción:
        voices_root() = data_root()/voices), de modo que borrarlas deja data_root
        vacío y comprobable.
        """
        hub = tmp_path / "hub"
        propio1 = hub / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        propio2 = hub / "models--ResembleAI--chatterbox"
        for d in (propio1, propio2):
            d.mkdir(parents=True)
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))

        data_root = tmp_path / "data_root"
        if voices_inside_root:
            voices = data_root / "voices"
        else:
            voices = tmp_path / "voices"
        (voices / "mi_voz").mkdir(parents=True)
        monkeypatch.setattr("tts_sidecar.paths.data_root", lambda: str(data_root))
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(voices))
        return propio1, propio2, voices

    def _fake_macos(self, monkeypatch, tmp_path):
        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))
        monkeypatch.setattr(sys, "platform", "darwin")
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        # Prefijo de Homebrew sin Caskroom por defecto (vía .dmg/one-liner).
        monkeypatch.setenv("HOMEBREW_PREFIX", str(tmp_path / "brew"))
        return home

    def _make_fake_app(self, tmp_path, subdir="Applications"):
        app = tmp_path / subdir / "tts-sidecar.app"
        exe = app / "Contents" / "MacOS" / "tts-sidecar"
        exe.parent.mkdir(parents=True)
        exe.write_bytes(b"bin")
        return app, exe

    def _fake_windows(self, monkeypatch, tmp_path,
                      quiet=r'"C:\Programs\tts-sidecar\unins000.exe" /SILENT',
                      key_present=True):
        import types
        monkeypatch.setattr(sys, "platform", "win32")
        monkeypatch.setattr(sys, "frozen", True, raising=False)
        exe = tmp_path / "Programs" / "tts-sidecar" / "tts-sidecar.exe"
        exe.parent.mkdir(parents=True)
        exe.write_bytes(b"exe")
        monkeypatch.setattr(sys, "executable", str(exe))

        fake = types.ModuleType("winreg")
        fake.HKEY_CURRENT_USER = "HKCU"

        class _Key:
            def __enter__(self):
                return self
            def __exit__(self, *a):
                return False

        if key_present:
            fake.OpenKey = lambda hive, sub: _Key()
            fake.QueryValueEx = lambda key, name: (quiet, 1)
        else:
            def _missing(*a):
                raise OSError("clave inexistente")
            fake.OpenKey = _missing
            fake.QueryValueEx = _missing
        monkeypatch.setitem(sys.modules, "winreg", fake)

        import subprocess
        popen = MagicMock()
        monkeypatch.setattr(subprocess, "Popen", popen)
        return exe, popen

    # ---- Parser / dispatch ---------------------------------------------------

    @pytest.mark.parametrize("conflicting", ["--remove-path", "--force-update"])
    def test_uninstall_es_mutuamente_excluyente(self, monkeypatch, capsys, conflicting):
        # argparse rechaza la combinación antes de despachar (SystemExit 2).
        from tts_sidecar.cli import main

        monkeypatch.setattr(sys, "argv", ["tts-sidecar", "setup", "--uninstall", conflicting])
        with pytest.raises(SystemExit) as exc:
            main()
        assert exc.value.code == 2
        assert "not allowed with" in capsys.readouterr().err

    def test_uninstall_plataforma_no_soportada_falla(self, monkeypatch, capsys):
        # Con el dispatch, darwin/win32 son ramas válidas; solo una plataforma
        # realmente fuera del dispatch (freebsd) cae en EXIT_NOT_APPLICABLE.
        from tts_sidecar.cli import cmd_setup, EXIT_NOT_APPLICABLE

        monkeypatch.setattr(sys, "frozen", True, raising=False)
        monkeypatch.setattr(sys, "platform", "freebsd")
        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True))
        assert exc.value.code == EXIT_NOT_APPLICABLE
        assert "no soporta la plataforma" in exc.value.message

    def test_uninstall_guard_canal_nativo(self, monkeypatch, capsys):
        # Proceso no congelado (fuente o pip/uv) → EXIT_NOT_APPLICABLE.
        from tts_sidecar.cli import cmd_setup, EXIT_NOT_APPLICABLE

        monkeypatch.setattr(sys, "platform", "linux")
        monkeypatch.setattr(sys, "frozen", False, raising=False)
        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True))
        assert exc.value.code == EXIT_NOT_APPLICABLE
        assert "pip uninstall" in exc.value.message

    def test_uninstall_json_requiere_yes(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup, EXIT_INVALID_INPUT

        self._fake_home_linux(monkeypatch, tmp_path)
        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True, json=True))
        assert exc.value.code == EXIT_INVALID_INPUT
        assert "requiere --yes" in exc.value.message

    # ---- Contrato compartido (rama Linux como representante) ------------------

    def test_uninstall_elimina_symlink_y_directorio(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home_linux(monkeypatch, tmp_path)
        self._fake_cleanup_env(tmp_path, monkeypatch)

        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)
        install_dir = home / ".local" / "opt" / "tts-sidecar"
        install_dir.mkdir(parents=True)
        (install_dir / "tts-sidecar-1.0.0-x86_64.AppImage").write_bytes(b"appimage")

        cmd_setup(MockArgs(uninstall=True, yes=True))

        assert not link.exists()
        assert not install_dir.exists()
        err = capsys.readouterr().err
        assert "Symlink eliminado" in err
        assert "Directorio de instalación eliminado" in err
        assert "Desinstalación completa" in err

    def test_uninstall_encadena_cleanup_con_yes(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup

        self._fake_home_linux(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)

        def _no_input(_):
            raise AssertionError("input() no debe llamarse con --yes")
        monkeypatch.setattr("builtins.input", _no_input)

        cmd_setup(MockArgs(uninstall=True, yes=True))

        assert not propio1.exists() and not propio2.exists() and not voices.exists()

    def test_uninstall_cancelacion_atomica(self, monkeypatch, tmp_path, capsys):
        # El reorden vuelve la cancelación atómica: cancelar el cleanup (primer
        # paso) aborta la desinstalación sin tocar PATH ni binario.
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home_linux(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)
        install_dir = home / ".local" / "opt" / "tts-sidecar"
        install_dir.mkdir(parents=True)
        monkeypatch.setattr("builtins.input", lambda _: "n")

        cmd_setup(MockArgs(uninstall=True))

        # Nada borrado: datos, symlink y directorio intactos.
        assert propio1.exists() and propio2.exists() and voices.exists()
        assert link.exists() and install_dir.exists()
        assert "cancelada" in capsys.readouterr().err.lower()

    def test_uninstall_nada_que_limpiar_continua(self, monkeypatch, tmp_path, capsys):
        # «No hay nada que limpiar» NO es cancelación: la desinstalación continúa
        # y borra symlink + directorio.
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home_linux(monkeypatch, tmp_path)
        # Entorno sin caché ni voices preexistentes.
        hub = tmp_path / "hub"
        hub.mkdir()
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        data_root = tmp_path / "data_root"
        data_root.mkdir()
        monkeypatch.setattr("tts_sidecar.paths.data_root", lambda: str(data_root))
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(data_root / "voices"))

        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)
        install_dir = home / ".local" / "opt" / "tts-sidecar"
        install_dir.mkdir(parents=True)

        cmd_setup(MockArgs(uninstall=True, yes=True))

        assert not link.exists()
        assert not install_dir.exists()

    def test_uninstall_json_payload_incluye_rutas_datos(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        import json as _json
        from tts_sidecar.cli import cmd_setup

        home = self._fake_home_linux(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)
        install_dir = home / ".local" / "opt" / "tts-sidecar"
        install_dir.mkdir(parents=True)

        cmd_setup(MockArgs(uninstall=True, yes=True, json=True))

        payload = _json.loads(capsys.readouterr().out.strip())
        assert payload["uninstall"] is True
        assert "schema_version" in payload
        # Rutas de datos del cleanup encadenado atestiguadas en removed.
        assert str(propio1) in payload["removed"]
        assert str(voices) in payload["removed"]
        # Symlink y directorio de instalación (borrados en proceso).
        assert str(link) in payload["removed"]
        assert str(install_dir) in payload["removed"]

    def test_uninstall_data_root_vacio_eliminado(self, monkeypatch, tmp_path, capsys):
        import json as _json
        from tts_sidecar.cli import cmd_setup

        self._fake_home_linux(monkeypatch, tmp_path)
        # Voces dentro de data_root: borrarlas deja data_root vacío.
        self._fake_cleanup_env(tmp_path, monkeypatch, voices_inside_root=True)
        data_root = tmp_path / "data_root"

        cmd_setup(MockArgs(uninstall=True, yes=True, json=True))

        payload = _json.loads(capsys.readouterr().out.strip())
        assert not data_root.exists()
        assert str(data_root) in payload["removed"]

    # ---- Rama macOS ----------------------------------------------------------

    def test_uninstall_macos_borra_bundle_symlink_cleanup(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_macos(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)
        app, exe = self._make_fake_app(tmp_path)
        monkeypatch.setattr(sys, "executable", str(exe))
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(tmp_path)

        cmd_setup(MockArgs(uninstall=True, yes=True))

        assert not app.exists()
        assert not link.exists()
        assert not propio1.exists() and not voices.exists()

    def test_uninstall_macos_resuelve_symlink_del_ejecutable(self, monkeypatch, tmp_path, capsys):
        if not _symlinks_supported(tmp_path):
            pytest.skip(_SYMLINK_SKIP_REASON)
        from tts_sidecar.cli import cmd_setup

        home = self._fake_macos(monkeypatch, tmp_path)
        self._fake_cleanup_env(tmp_path, monkeypatch)
        app, exe = self._make_fake_app(tmp_path)
        # sys.executable apunta al symlink de ~/.local/bin, no al binario real.
        link = home / ".local" / "bin" / "tts-sidecar"
        link.parent.mkdir(parents=True)
        link.symlink_to(exe)
        monkeypatch.setattr(sys, "executable", str(link))

        cmd_setup(MockArgs(uninstall=True, yes=True))

        # resolve() localizó el .app real pese al symlink del ejecutable.
        assert not app.exists()

    def test_uninstall_macos_fuera_de_app_falla(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup, EXIT_INVALID_INPUT

        self._fake_macos(monkeypatch, tmp_path)
        exe = tmp_path / "usr" / "local" / "bin" / "tts-sidecar"
        exe.parent.mkdir(parents=True)
        exe.write_bytes(b"bin")
        monkeypatch.setattr(sys, "executable", str(exe))

        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True, yes=True))
        assert exc.value.code == EXIT_NOT_APPLICABLE
        assert "bundle .app" in exc.value.message

    def test_uninstall_macos_homebrew_difiere_a_brew(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup, EXIT_STATE_CONFLICT

        self._fake_macos(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)
        app, exe = self._make_fake_app(tmp_path)
        monkeypatch.setattr(sys, "executable", str(exe))
        # Metadata del Caskroom presente bajo HOMEBREW_PREFIX.
        (tmp_path / "brew" / "Caskroom" / "tts-sidecar").mkdir(parents=True)

        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True, yes=True))
        assert exc.value.code == EXIT_STATE_CONFLICT
        assert "brew uninstall --cask --zap" in exc.value.message
        # Aborta sin borrar nada.
        assert app.exists()
        assert propio1.exists() and voices.exists()

    # ---- Rama Windows --------------------------------------------------------

    def test_uninstall_windows_valida_registro_y_desacopla(self, monkeypatch, tmp_path, capsys):
        import json as _json
        from tts_sidecar.cli import cmd_setup

        exe, popen = self._fake_windows(monkeypatch, tmp_path)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)

        cmd_setup(MockArgs(uninstall=True, yes=True, json=True))

        # cleanup corrió en proceso.
        assert not propio1.exists() and not voices.exists()
        # Desinstalador lanzado desacoplado, sin espera, con el string tal cual.
        popen.assert_called_once()
        assert popen.call_args[0][0] == r'"C:\Programs\tts-sidecar\unins000.exe" /SILENT'
        popen.return_value.wait.assert_not_called()

        payload = _json.loads(capsys.readouterr().out.strip())
        install_dir = str(exe.parent)
        assert str(propio1) in payload["removed"]
        assert payload["delegated"] == [install_dir]
        assert install_dir not in payload["removed"]

    def test_uninstall_windows_sin_registro_falla_sin_borrar(self, monkeypatch, tmp_path, capsys):
        from tts_sidecar.cli import cmd_setup, EXIT_INVALID_INPUT

        exe, popen = self._fake_windows(monkeypatch, tmp_path, key_present=False)
        propio1, propio2, voices = self._fake_cleanup_env(tmp_path, monkeypatch)

        with pytest.raises(CliError) as exc:
            cmd_setup(MockArgs(uninstall=True, yes=True))
        assert exc.value.code == EXIT_NOT_APPLICABLE
        # La validación del registro precede al cleanup: datos intactos.
        assert propio1.exists() and voices.exists()
        popen.assert_not_called()


class TestSpeechJSON:
    """speech say --json emite solo la voz efectiva a stdout (§2.10), idéntico
    en ruta directa y vía daemon."""

    def _args(self, **kw):
        kw.setdefault("timbre_reference", _make_wav("v.wav"))
        kw.setdefault("speech_reference", _make_wav("s.wav"))
        return MockArgs(**kw)

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.voices._resolve_voice_dir", return_value="/fake/mi_voz")
    @patch("tts_sidecar.voices.voice_paths")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_direct_json_payload(self, mock_engine_cls, _cached, mock_voice_paths, _resolve, mock_player_cls, capsys):
        import json
        from tts_sidecar.cli import cmd_speech_say, SCHEMA_VERSION

        mock_voice_paths.return_value = (_make_wav("v.wav"), _make_wav("s.wav"))
        engine = MagicMock()
        engine.synthesize.return_value = _synth_result(t3=1.5, s3gen=2.5)
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(
            no_daemon=True, json=True, voice="mi_voz",
            timbre_reference=None, speech_reference=None,
        ))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION,
            "voice": "mi_voz",
        }

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_daemon_json_payload(self, mock_running, mock_client_cls, _cached, mock_player_cls, capsys):
        import json
        from tts_sidecar.cli import cmd_speech_say, SCHEMA_VERSION

        client = MagicMock()
        client.synthesize.return_value = _synth_result(t3=3.0, s3gen=4.0)
        mock_client_cls.return_value = client

        cmd_speech_say(self._args(daemon=True, json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION,
            "voice": "default",
        }

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.engine.ChatterboxEngine")
    def test_without_json_stdout_stays_empty(self, mock_engine_cls, _cached, mock_player_cls, capsys):
        from tts_sidecar.cli import cmd_speech_say

        engine = MagicMock()
        engine.synthesize.return_value = _synth_result()
        mock_engine_cls.get_instance.return_value = engine

        cmd_speech_say(self._args(no_daemon=True, json=False))

        assert capsys.readouterr().out == ""


class TestWriteCommandsJSON:
    """Los cuatro comandos de escritura aceptan --json y emiten un único
    objeto JSON en stdout, con los listados informativos en stderr."""

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_voice_clone_json_payload(
        self, mock_register, _cached, mock_client_cls, _running, capsys
    ):
        import json
        from tts_sidecar.cli import cmd_voice_clone, SCHEMA_VERSION

        mock_register.return_value = ("/voices/nueva/timbre-reference.wav", "/voices/nueva/speech-reference.wav")
        mock_client_cls.return_value.precompute_voice.return_value = True

        cmd_voice_clone(MockArgs(name="nueva", json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION,
            "name": "nueva",
            "timbre": "/voices/nueva/timbre-reference.wav",
            "speech": "/voices/nueva/speech-reference.wav",
            "precomputed": True,
        }

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.voices.clone_voice_files")
    def test_voice_clone_json_payload_without_timbre(
        self, mock_register, _cached, mock_client_cls, _running, capsys
    ):
        """Sin timbre, el payload JSON representa la ausencia como null (decisión 4)."""
        import json
        from tts_sidecar.cli import cmd_voice_clone

        mock_register.return_value = (None, "/voices/nueva/speech-reference.wav")
        mock_client_cls.return_value.precompute_voice.return_value = True

        cmd_voice_clone(MockArgs(name="nueva", timbre_reference=None, json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload["timbre"] is None

    @patch("tts_sidecar.voices.remove_voice", return_value=True)
    def test_voice_remove_json_payload(self, _removed, capsys):
        import json
        from tts_sidecar.cli import cmd_voice_remove, SCHEMA_VERSION

        cmd_voice_remove(MockArgs(name="vieja", json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION,
            "name": "vieja",
            "removed": True,
        }

    def test_setup_json_payload_already_cached(self, monkeypatch, capsys):
        import json
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True):
            cli.cmd_setup(MockArgs(remove_path=False, json=True, language="es-latam"))

        out = capsys.readouterr().out
        payload = json.loads(out)
        assert payload["schema_version"] == cli.SCHEMA_VERSION
        assert payload["language"] == "es-latam"
        assert payload["models"]["es-mx-latam"]["already_cached"] is True
        assert payload["models"]["es-mx-latam"]["downloaded"] is False
        assert "cache_dir" in payload

    def test_setup_remove_path_json_payload(self, monkeypatch, tmp_path, capsys):
        import json
        from tts_sidecar.cli import cmd_setup, SCHEMA_VERSION

        home = tmp_path / "home"
        home.mkdir()
        monkeypatch.setattr(Path, "home", classmethod(lambda cls: home))

        cmd_setup(MockArgs(remove_path=True, json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION,
            "remove_path": True,
            "removed": False,
        }

    def _cleanup_env(self, tmp_path, monkeypatch):
        hub = tmp_path / "hub"
        propio1 = hub / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        propio2 = hub / "models--ResembleAI--chatterbox"
        for d in (propio1, propio2):
            d.mkdir(parents=True)
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
        voices = tmp_path / "voices"
        (voices / "mi_voz").mkdir(parents=True)
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(voices))
        # Aísla el almacén de habla sintética: sin este patch, `store_root()`
        # resuelve a la ruta real del usuario (data_root()/synthetic-speech) y
        # `cleanup --all --yes` borraría datos reales. Se deja inexistente para
        # que quede fuera de `removed` (cleanup filtra por existencia).
        store = tmp_path / "synthetic-speech"
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        return propio1, propio2, voices

    def _cleanup_args(self, **kw):
        import argparse
        return argparse.Namespace(
            model=kw.get("model", False),
            voices=kw.get("voices", False),
            all=kw.get("all", False),
            dry_run=kw.get("dry_run", False),
            yes=kw.get("yes", False),
            json=kw.get("json", False),
            cleanup_parser=MagicMock(),
        )

    def test_cleanup_json_with_yes_emits_removed_paths(self, tmp_path, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import cmd_cleanup, SCHEMA_VERSION

        propio1, propio2, voices = self._cleanup_env(tmp_path, monkeypatch)

        cmd_cleanup(self._cleanup_args(all=True, yes=True, json=True))

        captured = capsys.readouterr()
        payload = json.loads(captured.out)  # stdout: solo el objeto JSON
        assert payload["schema_version"] == SCHEMA_VERSION
        assert payload["dry_run"] is False
        assert sorted(payload["removed"]) == sorted(
            [str(propio1), str(propio2), str(voices)]
        )
        assert not propio1.exists() and not propio2.exists() and not voices.exists()
        assert "Rutas a eliminar" in captured.err  # listados informativos a stderr

    def test_cleanup_json_dry_run_lists_without_deleting(self, tmp_path, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import cmd_cleanup

        propio1, propio2, voices = self._cleanup_env(tmp_path, monkeypatch)

        cmd_cleanup(self._cleanup_args(all=True, dry_run=True, json=True))

        payload = json.loads(capsys.readouterr().out)
        assert payload["dry_run"] is True
        assert len(payload["removed"]) == 3
        assert propio1.exists() and propio2.exists() and voices.exists()

    def test_cleanup_json_without_yes_or_dry_run_exits_4(self, tmp_path, monkeypatch, capsys):
        from tts_sidecar.cli import cmd_cleanup, EXIT_INVALID_INPUT

        propio1, propio2, voices = self._cleanup_env(tmp_path, monkeypatch)

        with pytest.raises(CliError) as exc:
            cmd_cleanup(self._cleanup_args(all=True, json=True))

        assert exc.value.code == EXIT_INVALID_INPUT
        assert "--yes" in exc.value.message and "--dry-run" in exc.value.message
        assert propio1.exists() and propio2.exists() and voices.exists()


class TestDaemonVerbsJSON:
    """daemon start/stop/restart --json emiten un payload de acción
    ({"action"}, con "pid" cuando el manager lo expone)."""

    def _args(self, action, **kw):
        import argparse
        return argparse.Namespace(action=action, json=kw.get("json", True),
                                   autorestart=kw.get("autorestart", False),
                                   max_retries=kw.get("max_retries", None))

    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonManager")
    def test_start_json_payload_success(self, mock_manager_cls, _cached, capsys):
        import json
        from tts_sidecar.cli import cmd_daemon, SCHEMA_VERSION

        manager = MagicMock()
        manager.start.return_value = True
        manager._read_pid.return_value = 4242
        mock_manager_cls.return_value = manager

        cmd_daemon(self._args("start"))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION, "action": "start", "pid": 4242,
        }

    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonManager")
    def test_start_json_payload_failure_exits_5(self, mock_manager_cls, _cached, capsys):
        from tts_sidecar.cli import cmd_daemon, EXIT_DAEMON_UNREACHABLE

        manager = MagicMock()
        manager.start.return_value = False
        mock_manager_cls.return_value = manager

        with pytest.raises(CliError) as exc:
            cmd_daemon(self._args("start"))

        assert exc.value.code == EXIT_DAEMON_UNREACHABLE
        # En fallo no se emite payload de acción: main() emitirá solo el objeto
        # 'error'. Aquí, sin pasar por main(), stdout queda vacío.
        assert capsys.readouterr().out == ""

    @patch("tts_sidecar.daemon.DaemonManager")
    def test_stop_json_payload_success(self, mock_manager_cls, capsys):
        import json
        from tts_sidecar.cli import cmd_daemon, SCHEMA_VERSION

        manager = MagicMock()
        manager.stop.return_value = True
        mock_manager_cls.return_value = manager

        cmd_daemon(self._args("stop"))

        captured = capsys.readouterr()
        assert json.loads(captured.out) == {
            "schema_version": SCHEMA_VERSION, "action": "stop",
        }
        assert captured.err == ""

    @patch("tts_sidecar.daemon.DaemonManager")
    def test_stop_json_payload_failure_exits_5(self, mock_manager_cls, capsys):
        from tts_sidecar.cli import cmd_daemon, EXIT_DAEMON_UNREACHABLE

        manager = MagicMock()
        manager.stop.return_value = False
        mock_manager_cls.return_value = manager

        with pytest.raises(CliError) as exc:
            cmd_daemon(self._args("stop"))

        assert exc.value.code == EXIT_DAEMON_UNREACHABLE
        # En fallo no se emite payload de acción; stdout queda vacío.
        assert capsys.readouterr().out == ""

    @patch("tts_sidecar.daemon.DaemonManager")
    def test_restart_json_payload_success(self, mock_manager_cls, capsys):
        import json
        from tts_sidecar.cli import cmd_daemon, SCHEMA_VERSION

        manager = MagicMock()
        manager.restart.return_value = True
        manager._read_pid.return_value = 777
        mock_manager_cls.return_value = manager

        cmd_daemon(self._args("restart"))

        payload = json.loads(capsys.readouterr().out)
        assert payload == {
            "schema_version": SCHEMA_VERSION, "action": "restart", "pid": 777,
        }


class TestJsonChannelSingleObjectViaMain:
    """La ruta completa por main() emite exactamente UN objeto JSON en la salida
    no-cero de los cuatro comandos afectados: doctor (veredicto, exit 1) y
    daemon start/stop/restart (error, exit 5). json.loads sobre todo stdout
    fallaría si hubiera dos objetos concatenados."""

    def test_doctor_json_fail_single_object_via_main(self, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import main, EXIT_ERROR

        monkeypatch.setattr(sys, "argv", ["tts-sidecar", "doctor", "--json"])
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(SystemExit) as exc:
                main()

        assert exc.value.code == EXIT_ERROR
        payload = json.loads(capsys.readouterr().out)
        assert payload["failed"] > 0
        assert "error" not in payload

    def _daemon_main(self, monkeypatch, action, manager):
        from tts_sidecar.cli import main

        monkeypatch.setattr(sys, "argv", ["tts-sidecar", "daemon", action, "--json"])
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("tts_sidecar.daemon.DaemonManager", return_value=manager):
            with pytest.raises(SystemExit) as exc:
                main()
        return exc.value.code

    def test_daemon_start_fail_single_error_object_via_main(self, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import EXIT_DAEMON_UNREACHABLE

        manager = MagicMock()
        manager.start.return_value = False

        code = self._daemon_main(monkeypatch, "start", manager)

        assert code == EXIT_DAEMON_UNREACHABLE
        payload = json.loads(capsys.readouterr().out)
        assert set(payload.get("error", {})) == {"code", "reason", "message"}
        assert "action" not in payload

    def test_daemon_stop_fail_single_error_object_via_main(self, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import EXIT_DAEMON_UNREACHABLE

        manager = MagicMock()
        manager.stop.return_value = False

        code = self._daemon_main(monkeypatch, "stop", manager)

        assert code == EXIT_DAEMON_UNREACHABLE
        payload = json.loads(capsys.readouterr().out)
        assert set(payload.get("error", {})) == {"code", "reason", "message"}
        assert "action" not in payload

    def test_daemon_restart_fail_single_error_object_via_main(self, monkeypatch, capsys):
        import json
        from tts_sidecar.cli import EXIT_DAEMON_UNREACHABLE

        manager = MagicMock()
        manager.restart.return_value = False

        code = self._daemon_main(monkeypatch, "restart", manager)

        assert code == EXIT_DAEMON_UNREACHABLE
        payload = json.loads(capsys.readouterr().out)
        assert set(payload.get("error", {})) == {"code", "reason", "message"}
        assert "action" not in payload


class TestCmdSpeechEmptyText:
    def test_empty_text_is_rejected(self, capsys):
        from tts_sidecar.cli import cmd_speech_say

        with pytest.raises(CliError) as exc:
            cmd_speech_say(MockArgs(text="   "))

        assert "--text" in exc.value.message


class TestSchemaVersionJSON:
    """Todo payload --json incluye 'schema_version'."""

    def test_version_json_includes_schema_version(self, capsys):
        import json
        from tts_sidecar.cli import cmd_version, SCHEMA_VERSION

        cmd_version(MockArgs(json=True))
        payload = json.loads(capsys.readouterr().out)
        assert payload["schema_version"] == SCHEMA_VERSION
        assert payload["name"] == "tts-sidecar"

    def test_devices_json_includes_schema_version(self, capsys):
        import json
        from tts_sidecar.cli import cmd_devices, SCHEMA_VERSION

        with patch("tts_sidecar.audio.get_audio_devices", return_value=[]):
            cmd_devices(MockArgs(json=True))
        payload = json.loads(capsys.readouterr().out)
        assert payload["schema_version"] == SCHEMA_VERSION
        assert payload["devices"] == []

    def test_voice_list_json_includes_schema_version(self, capsys):
        import json
        from tts_sidecar.cli import cmd_voice_list, SCHEMA_VERSION

        with patch("tts_sidecar.voices.list_voices", return_value=["default"]):
            cmd_voice_list(MockArgs(json=True))
        payload = json.loads(capsys.readouterr().out)
        assert payload["schema_version"] == SCHEMA_VERSION
        assert payload["voices"] == ["default"]

    def test_doctor_json_includes_schema_version(self, capsys):
        import json
        from tts_sidecar.cli import cmd_doctor, SCHEMA_VERSION

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True):
            with patch("tts_sidecar.audio.get_audio_devices_with_status", return_value=([], False)):
                cmd_doctor(MockArgs(json=True))
        payload = json.loads(capsys.readouterr().out)
        assert payload["schema_version"] == SCHEMA_VERSION

    def test_doctor_json_fail_emits_single_object_verdict(self, capsys):
        """doctor --json con FAIL emite un solo objeto (el reporte) y sale por
        veredicto (return EXIT_ERROR), sin adjuntar objeto 'error'."""
        import json
        from tts_sidecar.cli import cmd_doctor, EXIT_ERROR

        # Modelo no cacheado ⇒ FAIL en el reporte.
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            result = cmd_doctor(MockArgs(json=True))

        assert result == EXIT_ERROR
        out = capsys.readouterr().out
        # Exactamente un objeto JSON: json.loads sobre todo stdout no falla.
        payload = json.loads(out)
        assert payload["failed"] > 0
        assert "error" not in payload

    def test_daemon_status_json_includes_schema_version(self, capsys):
        import argparse
        import json
        from tts_sidecar.cli import cmd_daemon, SCHEMA_VERSION

        args = argparse.Namespace(action="status", json=True)
        manager = MagicMock()
        manager.status.return_value = {"running": False}
        with patch("tts_sidecar.daemon.DaemonManager", return_value=manager):
            cmd_daemon(args)
        payload = json.loads(capsys.readouterr().out)
        assert payload["schema_version"] == SCHEMA_VERSION
        assert payload["running"] is False


# Comandos --json cubiertos por tests dedicados (TestSpeechJSON, TestDaemonVerbsJSON,
# TestWriteCommandsJSON, TestSchemaVersionJSON). Es la única lista mantenida a mano
# de este contrato: TestJSONContractStructure la compara contra lo que el propio
# parser declara, así que un comando --json nuevo sin añadir aquí (o viceversa)
# hace fallar el test, en vez de quedar fuera de la cobertura en silencio.
_JSON_COVERED_COMMANDS = {
    "speech synthesize", "speech say", "speech play", "speech list", "speech remove",
    "devices", "doctor", "setup", "cleanup", "version",
    "voice list", "voice clone", "voice remove",
    "daemon start", "daemon stop", "daemon restart", "daemon status",
}

# 'daemon serve' es la única exclusión deliberada: su contrato es el stream
# NDJSON de /synthesize (protocol.py), no un payload --json de una sola línea.
_JSON_DELIBERATELY_EXCLUDED = {"daemon serve"}


class TestJSONContractStructure:
    """Descubre desde build_parser() (la fuente de verdad real del CLI) qué
    subcomandos declaran --json, y lo compara contra _JSON_COVERED_COMMANDS.
    Protección bidireccional: un comando --json nuevo sin cobertura, o un flag
    --json retirado de un comando ya cubierto, rompen este test."""

    @staticmethod
    def _has_json_flag(subparser) -> bool:
        import argparse
        return any(
            action.dest == "json" and isinstance(action, argparse._StoreTrueAction)
            for action in subparser._actions
        )

    @classmethod
    def _discover_json_commands(cls, parser) -> set:
        import argparse
        from tts_sidecar.cli import top_level_subparsers

        discovered = set()
        top = top_level_subparsers(parser)
        for name, sub in top.choices.items():
            nested = next(
                (a for a in sub._actions if isinstance(a, argparse._SubParsersAction)),
                None,
            )
            if nested is not None:
                for subname, subsub in nested.choices.items():
                    if cls._has_json_flag(subsub):
                        discovered.add(f"{name} {subname}")
            elif cls._has_json_flag(sub):
                discovered.add(name)
        return discovered

    def test_discovered_commands_match_declared_coverage(self):
        from tts_sidecar.cli import build_parser

        discovered = self._discover_json_commands(build_parser())
        assert discovered == _JSON_COVERED_COMMANDS, (
            "El parser real declara --json en un conjunto de comandos distinto "
            "al de _JSON_COVERED_COMMANDS. Si añadiste/quitaste un --json, "
            "actualiza esa constante (y sus tests dedicados) en test_cli.py.\n"
            f"En el parser pero no cubiertos: {discovered - _JSON_COVERED_COMMANDS}\n"
            f"Cubiertos pero ausentes del parser: {_JSON_COVERED_COMMANDS - discovered}"
        )

    def test_daemon_serve_has_no_json_flag(self):
        """Exclusión deliberada: 'daemon serve' no es un comando --json de una
        sola línea (su contrato es el stream NDJSON), así que no debe aparecer
        ni en el parser con --json ni en la cobertura declarada."""
        from tts_sidecar.cli import build_parser, top_level_subparsers
        import argparse

        top = top_level_subparsers(build_parser())
        daemon_sub = top.choices["daemon"]
        nested = next(
            a for a in daemon_sub._actions if isinstance(a, argparse._SubParsersAction)
        )
        serve_parser = nested.choices["serve"]
        assert not self._has_json_flag(serve_parser)
        assert "daemon serve" not in _JSON_COVERED_COMMANDS
        assert "daemon serve" in _JSON_DELIBERATELY_EXCLUDED

    @pytest.mark.parametrize("command", sorted(_JSON_COVERED_COMMANDS))
    def test_covered_commands_exist_in_parser(self, command):
        """Cada entrada declarada en _JSON_COVERED_COMMANDS corresponde a un
        subcomando real del parser (no un nombre obsoleto tras un rename)."""
        from tts_sidecar.cli import build_parser, top_level_subparsers
        import argparse

        top = top_level_subparsers(build_parser())
        parts = command.split(" ")
        if len(parts) == 1:
            assert parts[0] in top.choices
        else:
            group, action = parts
            assert group in top.choices
            nested = next(
                a for a in top.choices[group]._actions
                if isinstance(a, argparse._SubParsersAction)
            )
            assert action in nested.choices


class TestSpeechLongText:
    """Un texto muy largo emite una advertencia (no bloqueante) a stderr."""

    def test_long_text_warns_and_continues(self, capsys):
        from tts_sidecar.cli import cmd_speech_say

        largo = "a" * 2500
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(CliError):
                cmd_speech_say(MockArgs(text=largo, no_daemon=True))
        assert "Advertencia" in capsys.readouterr().err

    def test_short_text_does_not_warn(self, capsys):
        from tts_sidecar.cli import cmd_speech_say

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False):
            with pytest.raises(CliError):
                cmd_speech_say(MockArgs(text="Hola mundo", no_daemon=True))
        assert "Advertencia" not in capsys.readouterr().err


class TestSingleTextLimit:
    """texto > MAX_TEXT_LENGTH falla con exit 4 antes de cualquier despacho."""

    def test_text_exceeds_max_text_length_exits_2_without_daemon(self, capsys):
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT
        from tts_sidecar.daemon.protocol import MAX_TEXT_LENGTH

        demasiado_largo = "a" * (MAX_TEXT_LENGTH + 1)
        with pytest.raises(CliError) as exc_info:
            cmd_speech_say(MockArgs(text=demasiado_largo, no_daemon=True))
        assert exc_info.value.code == EXIT_INVALID_INPUT
        assert str(MAX_TEXT_LENGTH) in exc_info.value.message

    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_text_exceeds_max_text_length_exits_2_with_daemon(self, _running, capsys):
        from tts_sidecar.cli import cmd_speech_say, EXIT_INVALID_INPUT
        from tts_sidecar.daemon.protocol import MAX_TEXT_LENGTH

        demasiado_largo = "a" * (MAX_TEXT_LENGTH + 1)
        with pytest.raises(CliError) as exc_info:
            cmd_speech_say(MockArgs(text=demasiado_largo))
        assert exc_info.value.code == EXIT_INVALID_INPUT


class TestComputeBackendIgnoredViaDaemon:
    """--compute-backend explícito con daemon activo emite un warning."""

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_backend_non_auto_with_explicit_daemon_warns(
        self, mock_running, mock_client_cls, _cached, mock_player_cls, capsys
    ):
        from tts_sidecar.cli import cmd_speech_say

        mock_client_cls.return_value.synthesize.return_value = _synth_result(b"RIFF....")
        cmd_speech_say(MockArgs(daemon=True, compute_backend="cuda"))
        assert "--compute-backend" in capsys.readouterr().err

    @patch("tts_sidecar.audio.AudioPlayer")
    @patch("tts_sidecar.model_cache.is_model_cached", return_value=True)
    @patch("tts_sidecar.daemon.DaemonIPCClient")
    @patch("tts_sidecar.daemon.is_daemon_running", return_value=True)
    def test_backend_auto_with_daemon_does_not_warn(
        self, mock_running, mock_client_cls, _cached, mock_player_cls, capsys
    ):
        from tts_sidecar.cli import cmd_speech_say

        mock_client_cls.return_value.synthesize.return_value = _synth_result(b"RIFF....")
        cmd_speech_say(MockArgs(daemon=True, compute_backend="auto"))
        assert "--compute-backend" not in capsys.readouterr().err


class TestVoiceAddWithoutComputeBackend:
    """voice clone --compute-backend ya no existe (flag muerta eliminada)."""

    def test_parser_rejects_compute_backend(self, monkeypatch, capsys):
        from tts_sidecar.cli import main
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr(sys, "argv", [
            "tts-sidecar", "voice", "clone", "--name", "x", "--timbre-reference", "r.wav",
            "--speech-reference", "s.wav", "--compute-backend", "cuda",
        ])
        with pytest.raises(SystemExit) as exc:
            main()
        assert exc.value.code == EXIT_INVALID_INPUT
        assert "unrecognized" in capsys.readouterr().err.lower()


class TestVoiceCloneParserWithoutTimbre:
    """voice clone acepta omitir --timbre-reference (timbre opcional, decisión 1)."""

    def test_parser_accepts_missing_timbre_reference(self):
        from tts_sidecar.cli import build_parser

        parser = build_parser()
        args = parser.parse_args([
            "voice", "clone", "--name", "X", "--speech-reference", "s.wav",
        ])
        assert args.timbre_reference is None


class TestDescribeProvisionFailure:
    """_describe_provision_failure clasifica el fallo en la terna (code, reason, message).

    Las cuatro familias de precondición salen con EXIT_PRECONDITION_FAILED (8) y
    su reason; lo que no encaja se queda en EXIT_ERROR (1) como fallo genérico.
    """

    def test_gated_repo_es_credentials_8(self):
        from tts_sidecar.cli import _describe_provision_failure, EXIT_PRECONDITION_FAILED
        from huggingface_hub.errors import GatedRepoError

        resp = MagicMock()
        resp.status_code = 403
        code, reason, message = _describe_provision_failure(
            GatedRepoError("gated", response=resp))
        assert code == EXIT_PRECONDITION_FAILED
        assert reason == "credentials"
        assert message.startswith("[FAIL]")

    def test_http_401_403_es_credentials_8(self):
        from tts_sidecar.cli import _describe_provision_failure, EXIT_PRECONDITION_FAILED
        from huggingface_hub.errors import HfHubHTTPError

        resp = MagicMock()
        resp.status_code = 401
        code, reason, _ = _describe_provision_failure(
            HfHubHTTPError("401", response=resp))
        assert code == EXIT_PRECONDITION_FAILED
        assert reason == "credentials"

    def test_request_exception_es_network_8(self):
        from tts_sidecar.cli import _describe_provision_failure, EXIT_PRECONDITION_FAILED
        import requests

        code, reason, _ = _describe_provision_failure(
            requests.exceptions.ConnectionError("sin red"))
        assert code == EXIT_PRECONDITION_FAILED
        assert reason == "network"

    def test_permission_error_es_permissions_8(self):
        from tts_sidecar.cli import _describe_provision_failure, EXIT_PRECONDITION_FAILED

        code, reason, _ = _describe_provision_failure(PermissionError("denegado"))
        assert code == EXIT_PRECONDITION_FAILED
        assert reason == "permissions"

    def test_enospc_es_disk_full_8(self):
        import errno
        from tts_sidecar.cli import _describe_provision_failure, EXIT_PRECONDITION_FAILED

        e = OSError("sin espacio")
        e.errno = errno.ENOSPC
        code, reason, _ = _describe_provision_failure(e)
        assert code == EXIT_PRECONDITION_FAILED
        assert reason == "disk_full"

    def test_generico_se_queda_en_error_1(self):
        from tts_sidecar.cli import _describe_provision_failure, EXIT_ERROR

        code, reason, _ = _describe_provision_failure(ValueError("desconocido"))
        assert code == EXIT_ERROR
        assert reason == "provision_failed"


class TestDoctorRAM:
    """doctor incluye un chequeo de RAM advisory (WARN) que no penaliza."""

    def test_low_ram_gives_warn(self, capsys):
        import tts_sidecar.cli as cli

        fake_mem = MagicMock()
        fake_mem.total = 4 * 1024 ** 3
        with patch.object(cli, "_environment_checks", return_value=[]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=False))
        out = capsys.readouterr().out
        assert "[WARN] RAM" in out

    def test_sufficient_ram_gives_pass(self, capsys):
        import tts_sidecar.cli as cli

        fake_mem = MagicMock()
        fake_mem.total = 16 * 1024 ** 3
        with patch.object(cli, "_environment_checks", return_value=[]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=False))
        out = capsys.readouterr().out
        assert "[PASS] RAM" in out

    def test_ram_warn_does_not_alter_exit_code(self, capsys):
        import tts_sidecar.cli as cli

        fake_mem = MagicMock()
        fake_mem.total = 2 * 1024 ** 3
        with patch.object(cli, "_environment_checks",
                          return_value=[("PASS", "Chatterbox TTS", "0.1.7")]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=False))

    def test_ram_in_json_appears_as_check_with_status_warn(self, capsys):
        import json
        import tts_sidecar.cli as cli

        fake_mem = MagicMock()
        fake_mem.total = 4 * 1024 ** 3
        with patch.object(cli, "_environment_checks", return_value=[]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=True))
        payload = json.loads(capsys.readouterr().out)
        ram = next(c for c in payload["checks"] if c["name"] == "RAM")
        assert ram["status"] == "WARN"
        assert payload["failed"] == 0


class TestDoctorOnedrive:
    """doctor incluye el chequeo OneDrive user-data-dir (WARN advisory en
    Windows). No altera el exit code (failed == 0)."""

    def _patch_onedrive(self, monkeypatch, data_root_path):
        monkeypatch.setattr(sys, "platform", "win32")
        monkeypatch.setattr(
            "tts_sidecar.paths.data_root", lambda: data_root_path)
        monkeypatch.delenv("OneDrive", raising=False)
        monkeypatch.delenv("OneDriveCommercial", raising=False)
        monkeypatch.setenv("OneDrive", r"C:\Users\test\OneDrive")

    def test_windows_onedrive_warns_in_json(self, monkeypatch, capsys):
        import json
        import tts_sidecar.cli as cli

        self._patch_onedrive(monkeypatch, r"C:\Users\test\OneDrive\tts-sidecar")

        fake_mem = MagicMock()
        fake_mem.total = 16 * 1024 ** 3
        with patch.object(cli, "_environment_checks", return_value=[]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=True))

        payload = json.loads(capsys.readouterr().out)
        onedrive = next(
            c for c in payload["checks"] if c["name"] == "OneDrive user-data-dir")
        assert onedrive["status"] == "WARN"
        assert payload["failed"] == 0

    def test_windows_onedrive_warn_printed_human(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        self._patch_onedrive(monkeypatch, r"C:\Users\test\OneDrive\tts-sidecar")

        fake_mem = MagicMock()
        fake_mem.total = 16 * 1024 ** 3
        with patch.object(cli, "_environment_checks", return_value=[]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("psutil.virtual_memory", return_value=fake_mem):
            cli.cmd_doctor(MockArgs(json=False))

        out = capsys.readouterr().out
        assert "[WARN] OneDrive user-data-dir" in out


class TestSetupDiskAndForceUpdate:
    """Pre-chequeo de disco y --force-update en setup."""

    def test_insufficient_disk_aborts_before_download(self, monkeypatch, capsys):
        import shutil
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        poco = shutil._ntuple_diskusage(total=10 * 1024 ** 3, used=9 * 1024 ** 3,
                                        free=1 * 1024 ** 3)
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("shutil.disk_usage", return_value=poco):
            with pytest.raises(CliError) as exc:
                cli.cmd_setup(MockArgs(remove_path=False, language="es-latam"))
        assert exc.value.code == cli.EXIT_PRECONDITION_FAILED
        assert "Espacio en disco insuficiente" in exc.value.message

    def test_disk_not_checked_if_already_cached(self, monkeypatch, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=True), \
                patch("shutil.disk_usage", side_effect=AssertionError("no debe llamarse")):
            cli.cmd_setup(MockArgs(remove_path=False))
        assert "Provisión completa" in capsys.readouterr().err

    def test_force_update_deletes_model_snapshots(self, monkeypatch, tmp_path, capsys):
        import shutil
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        model_dir = tmp_path / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        (model_dir / "snapshots").mkdir(parents=True)
        (model_dir / "snapshots" / "marca.txt").write_text("x", encoding="utf-8")

        poco = shutil._ntuple_diskusage(total=10 * 1024 ** 3, used=10 * 1024 ** 3,
                                        free=0)
        with patch("tts_sidecar.model_cache.model_cache_dirs", return_value=[model_dir]), \
                patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("shutil.disk_usage", return_value=poco):
            with pytest.raises(CliError):
                cli.cmd_setup(MockArgs(remove_path=False, force_update=True, language="es-latam"))

        assert not model_dir.exists()
        assert "force-update" in capsys.readouterr().err


class TestSetupLightDownload:
    """setup descarga vía snapshot_download, sin instanciar ChatterboxEngine."""

    def test_setup_downloads_without_instantiating_engine(self, monkeypatch, tmp_path, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        mucho = __import__("shutil")._ntuple_diskusage(
            total=100 * 1024 ** 3, used=1 * 1024 ** 3, free=99 * 1024 ** 3
        )
        mock_snapshot_download = MagicMock(return_value=str(tmp_path))
        mock_get_instance = MagicMock(
            side_effect=AssertionError("setup no debe instanciar ChatterboxEngine")
        )

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("tts_sidecar.model_cache.is_ve_cached", return_value=True), \
                patch("shutil.disk_usage", return_value=mucho), \
                patch("huggingface_hub.snapshot_download", mock_snapshot_download), \
                patch("tts_sidecar.engine.ChatterboxEngine.get_instance", mock_get_instance):
            cli.cmd_setup(MockArgs(remove_path=False, language="es-latam"))

        mock_snapshot_download.assert_called_once()
        assert mock_snapshot_download.call_args.kwargs["repo_id"] == (
            "ResembleAI/Chatterbox-Multilingual-es-mx-latam"
        )
        mock_get_instance.assert_not_called()
        assert "descargado(s) correctamente" in capsys.readouterr().err


class TestSetupMultiLanguage:
    """setup --language {es-latam, en, all}: rediseño cross-lingual."""

    def test_default_language_downloads_both_models(self, monkeypatch, tmp_path, capsys):
        """Sin --language, el default 'all' descarga ambos modelos."""
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        mucho = __import__("shutil")._ntuple_diskusage(
            total=100 * 1024 ** 3, used=1 * 1024 ** 3, free=99 * 1024 ** 3
        )
        mock_snapshot_download = MagicMock(return_value=str(tmp_path))

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("tts_sidecar.model_cache.is_ve_cached", return_value=True), \
                patch("shutil.disk_usage", return_value=mucho), \
                patch("huggingface_hub.snapshot_download", mock_snapshot_download):
            cli.cmd_setup(MockArgs(remove_path=False))

        assert mock_snapshot_download.call_count == 2
        repo_ids = {c.kwargs["repo_id"] for c in mock_snapshot_download.call_args_list}
        assert repo_ids == {
            "ResembleAI/Chatterbox-Multilingual-es-mx-latam",
            "ResembleAI/chatterbox",
        }

    def test_language_en_downloads_only_english_base(self, monkeypatch, tmp_path, capsys):
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        mucho = __import__("shutil")._ntuple_diskusage(
            total=100 * 1024 ** 3, used=1 * 1024 ** 3, free=99 * 1024 ** 3
        )
        mock_snapshot_download = MagicMock(return_value=str(tmp_path))

        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("shutil.disk_usage", return_value=mucho), \
                patch("huggingface_hub.snapshot_download", mock_snapshot_download):
            cli.cmd_setup(MockArgs(remove_path=False, language="en"))

        mock_snapshot_download.assert_called_once()
        assert mock_snapshot_download.call_args.kwargs["repo_id"] == "ResembleAI/chatterbox"

    def test_disk_check_scales_with_pending_model_count(self, monkeypatch, capsys):
        """Con 'all' pendiente (2 modelos), el umbral de disco se duplica."""
        import shutil
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        # Suficiente para un modelo pero no para dos.
        justo = shutil._ntuple_diskusage(
            total=10 * 1024 ** 3,
            used=4 * 1024 ** 3,
            free=cli.MIN_FREE_DISK_BYTES + 1,
        )
        with patch("tts_sidecar.model_cache.is_model_cached", return_value=False), \
                patch("shutil.disk_usage", return_value=justo):
            with pytest.raises(CliError) as exc:
                cli.cmd_setup(MockArgs(remove_path=False))
        assert exc.value.code == cli.EXIT_PRECONDITION_FAILED
        assert "Espacio en disco insuficiente" in exc.value.message

    def test_partial_cache_only_downloads_missing_model(self, monkeypatch, tmp_path, capsys):
        """es-mx-latam ya cacheado + en ausente: solo se descarga en."""
        import tts_sidecar.cli as cli

        monkeypatch.setattr(
            cli, "_environment_checks",
            lambda: [("PASS", "Chatterbox TTS", "0.1.7")],
        )
        mucho = __import__("shutil")._ntuple_diskusage(
            total=100 * 1024 ** 3, used=1 * 1024 ** 3, free=99 * 1024 ** 3
        )
        mock_snapshot_download = MagicMock(return_value=str(tmp_path))

        def fake_cached(model):
            return model == "es-mx-latam"

        with patch("tts_sidecar.model_cache.is_model_cached", side_effect=fake_cached), \
                patch("shutil.disk_usage", return_value=mucho), \
                patch("huggingface_hub.snapshot_download", mock_snapshot_download):
            cli.cmd_setup(MockArgs(remove_path=False))

        mock_snapshot_download.assert_called_once()
        assert mock_snapshot_download.call_args.kwargs["repo_id"] == "ResembleAI/chatterbox"


class TestBootstrap:
    """El bootstrap pre-import (bootstrap.apply()) debe correr en cualquier vía
    de invocación del proceso, ser idempotente y no crashear con pkg_resources
    ausente (Python 3.13+)."""

    def _reset(self, monkeypatch):
        from tts_sidecar import bootstrap
        monkeypatch.setattr(bootstrap, "_applied", False)
        return bootstrap

    def test_apply_is_idempotent(self, monkeypatch):
        bootstrap = self._reset(monkeypatch)
        calls = []
        monkeypatch.setattr(
            bootstrap, "_install_pkg_resources_mock", lambda: calls.append(1)
        )

        bootstrap.apply()
        bootstrap.apply()

        assert calls == [1]

    def test_apply_sets_expected_env_vars(self, monkeypatch):
        bootstrap = self._reset(monkeypatch)
        for var in (
            "HF_HUB_DISABLE_IMPLICIT_TOKEN",
            "TRANSFORMERS_VERBOSITY", "TRANSFORMERS_NO_ADVISORY_WARNINGS",
            "TOKENIZERS_PARALLELISM",
        ):
            monkeypatch.delenv(var, raising=False)

        bootstrap.apply()

        assert os.environ["HF_HUB_DISABLE_IMPLICIT_TOKEN"] == "1"
        assert os.environ["TRANSFORMERS_VERBOSITY"] == "error"
        assert os.environ["TRANSFORMERS_NO_ADVISORY_WARNINGS"] == "1"
        assert os.environ["TOKENIZERS_PARALLELISM"] == "false"

    # -- allow-list de warnings en vez de catch-all -------------

    def test_apply_has_no_catch_all_warning_filter(self, monkeypatch):
        """No debe instalarse ningún filtro catch-all (action='ignore',
        message='', module='', category=Warning) que silencie todo."""
        import warnings as _warnings

        bootstrap = self._reset(monkeypatch)
        # Aislar: arrancar desde una lista de filtros vacía.
        monkeypatch.setattr(_warnings, "filters", [])
        # `apply()` es idempotente: desactivarlo para poder reaplicar limpio.
        bootstrap._applied = False
        bootstrap.apply()

        # `warnings.filters` almacena los message/module como regex compiladas.
        def _pattern(value):
            return value.pattern if hasattr(value, "pattern") else value

        # Ninguna entrada residual debe ser un silencio global.
        for entry in _warnings.filters:
            action, _msg, _cat, _mod, _ln = entry
            assert not (
                action == "ignore"
                and _pattern(_msg) == ""
                and _pattern(_mod) == ""
                and _cat is Warning
            )

    def test_silenced_warnings_allow_list_has_expected_entries(self, monkeypatch):
        """La allow-list `_SILENCED_WARNINGS` declara las tres supresiones
        benignas conocidas: pkg_resources, diffusers (LoRACompatibleLinear) y
        torch (sdp_kernel, filtrado por mensaje porque el stacklevel de
        PyTorch apunta a contextlib)."""
        bootstrap = self._reset(monkeypatch)
        assert (
            "pkg_resources is deprecated", Warning, None
        ) in bootstrap._SILENCED_WARNINGS
        assert (
            None, FutureWarning, r"^diffusers\."
        ) in bootstrap._SILENCED_WARNINGS
        assert (
            r".*torch\.backends\.cuda\.sdp_kernel", FutureWarning, None
        ) in bootstrap._SILENCED_WARNINGS

    def test_apply_does_not_silence_unrelated_warnings(self, monkeypatch):
        """Un UserWarning de control debe propagar (quedar registrado), probando
        que el catch-all global desapareció."""
        import warnings as _warnings

        bootstrap = self._reset(monkeypatch)
        monkeypatch.setattr(_warnings, "filters", [])
        bootstrap._applied = False
        bootstrap.apply()

        with _warnings.catch_warnings(record=True) as recorded:
            _warnings.simplefilter("always")
            _warnings.warn("control allow-list", UserWarning)

        assert any(
            w.category is UserWarning and "control allow-list" in str(w.message)
            for w in recorded
        )

    def test_apply_reconfigures_streams_to_utf8(self, monkeypatch):
        """La reconfiguración UTF-8 de stdout/stderr, antes en el nivel de
        módulo de cli.py, es parte de la capa única bootstrap.apply(), de modo
        que las tres vías de entrada (pip/bin/-m) y el daemon heredan el mismo
        contrato de codificación."""
        bootstrap = self._reset(monkeypatch)

        class FakeStream:
            def __init__(self):
                self.encoding_set = None

            def reconfigure(self, encoding=None, **kwargs):
                self.encoding_set = encoding

        out, err = FakeStream(), FakeStream()
        monkeypatch.setattr(sys, "stdout", out)
        monkeypatch.setattr(sys, "stderr", err)

        bootstrap.apply()

        assert out.encoding_set == "utf-8"
        assert err.encoding_set == "utf-8"

    def test_apply_survives_unreconfigurable_stream(self, monkeypatch):
        """Un stream cuyo reconfigure lanza (ya leído/cerrado) no aborta el
        arranque: apply() sigue fijando las env vars."""
        bootstrap = self._reset(monkeypatch)
        monkeypatch.delenv("TOKENIZERS_PARALLELISM", raising=False)

        class HostileStream:
            def reconfigure(self, encoding=None, **kwargs):
                raise ValueError("no se puede cambiar el encoding")

        monkeypatch.setattr(sys, "stdout", HostileStream())
        monkeypatch.setattr(sys, "stderr", HostileStream())

        bootstrap.apply()  # no debe propagar

        assert os.environ["TOKENIZERS_PARALLELISM"] == "false"

    def test_installs_pkg_resources_mock_with_valid_spec_when_absent(self, monkeypatch):
        bootstrap = self._reset(monkeypatch)
        sys.modules.pop("pkg_resources", None)
        monkeypatch.setattr(
            bootstrap.importlib.util, "find_spec",
            lambda name: None if name == "pkg_resources" else object(),
        )

        bootstrap.apply()

        mock = sys.modules["pkg_resources"]
        assert mock.__spec__ is not None
        assert callable(mock.resource_filename)
        del sys.modules["pkg_resources"]

    def test_does_not_reinstall_mock_when_pkg_resources_already_present(self, monkeypatch):
        bootstrap = self._reset(monkeypatch)
        sentinel = object()
        monkeypatch.setitem(sys.modules, "pkg_resources", sentinel)

        bootstrap.apply()

        assert sys.modules["pkg_resources"] is sentinel

    # -- resource_filename del mock instalado, sus tres ramas -------

    def _install_mock(self, bootstrap, monkeypatch):
        """Instala el mock (find_spec('pkg_resources') -> None durante apply)
        y devuelve el módulo mockeado para invocar resource_filename directamente."""
        sys.modules.pop("pkg_resources", None)
        monkeypatch.setattr(bootstrap.importlib.util, "find_spec", lambda name: None)
        bootstrap.apply()
        return sys.modules["pkg_resources"]

    def test_resource_filename_falls_back_to_bare_resource_when_spec_is_none(self, monkeypatch):
        """Paquete no resoluble (find_spec devuelve None): sin __spec__ no hay
        directorio base, así que se retorna el recurso tal cual se pidió."""
        bootstrap = self._reset(monkeypatch)
        mock = self._install_mock(bootstrap, monkeypatch)
        try:
            assert mock.resource_filename("paquete.inexistente", "datos/archivo.wav") == "datos/archivo.wav"
        finally:
            sys.modules.pop("pkg_resources", None)

    def test_resource_filename_falls_back_when_spec_has_no_search_locations(self, monkeypatch):
        """Spec válido pero sin submodule_search_locations (módulo simple, no
        paquete): tampoco hay directorio base resoluble."""
        import types

        bootstrap = self._reset(monkeypatch)
        mock = self._install_mock(bootstrap, monkeypatch)
        try:
            fake_spec = types.SimpleNamespace(submodule_search_locations=None)
            monkeypatch.setattr(bootstrap.importlib.util, "find_spec", lambda name: fake_spec)
            assert mock.resource_filename("algun.modulo", "data.wav") == "data.wav"
        finally:
            sys.modules.pop("pkg_resources", None)

    def test_resource_filename_falls_back_when_search_locations_is_empty(self, monkeypatch):
        """submodule_search_locations existe pero está vacía: mismo fallback
        que None, ya que la condición es una comprobación de veracidad."""
        import types

        bootstrap = self._reset(monkeypatch)
        mock = self._install_mock(bootstrap, monkeypatch)
        try:
            fake_spec = types.SimpleNamespace(submodule_search_locations=[])
            monkeypatch.setattr(bootstrap.importlib.util, "find_spec", lambda name: fake_spec)
            assert mock.resource_filename("algun.paquete", "data.wav") == "data.wav"
        finally:
            sys.modules.pop("pkg_resources", None)

    def test_resource_filename_resolves_path_when_spec_has_search_locations(self, monkeypatch, tmp_path):
        """Paquete resoluble con directorio base: arma la ruta absoluta
        uniendo la primera search location con el recurso pedido."""
        import types

        bootstrap = self._reset(monkeypatch)
        mock = self._install_mock(bootstrap, monkeypatch)
        try:
            fake_spec = types.SimpleNamespace(submodule_search_locations=[str(tmp_path)])
            monkeypatch.setattr(bootstrap.importlib.util, "find_spec", lambda name: fake_spec)
            result = mock.resource_filename("tts_sidecar", "voices/default/timbre-reference.wav")
            assert result == str(tmp_path / "voices/default/timbre-reference.wav")
        finally:
            sys.modules.pop("pkg_resources", None)


# ============================================================================
# Tests de matriz del grupo `speech` (Tareas 2–6)
# ============================================================================

class TestCmdSpeechSynthesize:
    """Matriz de `speech synthesize`: una fila por regla de validación (§2.6)
    y por caso de la matriz de comportamiento (§2.7)."""

    def _fake_env(self, tmp_path, monkeypatch):
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        return store

    def test_success_persists_wav_and_sidecar(self, tmp_path, monkeypatch):
        """Filas de éxito: persiste WAV y sidecar, exit 0."""
        store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.1, s3gen=2.2), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        args = MockArgs(text="hola", label="saludo", voice="default")
        cmd_speech_synthesize(args)

        wav = store / "default" / "saludo.wav"
        assert wav.exists()
        assert wav.with_suffix(".json").exists()

    def test_json_payload_exact_keys(self, tmp_path, monkeypatch):
        """Filas JSON: el payload contiene exactamente las 5 claves de §2.10."""
        store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=0.5, s3gen=0.3), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        emitted = {}
        monkeypatch.setattr("tts_sidecar.cli.emit_json", lambda p: emitted.update(p))

        args = MockArgs(text="hola", label="saludo", voice="default", json=True)
        cmd_speech_synthesize(args)

        assert set(emitted.keys()) == {"voice", "label", "t3_time", "s3gen_time", "daemon"}
        assert emitted["voice"] == "default"
        assert emitted["label"] == "saludo"
        assert emitted["t3_time"] == 0.5
        assert emitted["s3gen_time"] == 0.3
        assert emitted["daemon"] is False

    def test_collision_without_force_exits_6_and_does_not_synthesize(self, tmp_path, monkeypatch):
        """Fast-fail de colisión: exit 6 SIN llamar a _dispatch_synthesis."""
        store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_STATE_CONFLICT

        synthetic_speech.save("default", "saludo", b"RIFFold", "texto")

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        dispatch_called = [False]

        def _never(*_, **__):
            dispatch_called[0] = True
            return (_synth_result(b"RIFF"), False)

        monkeypatch.setattr("tts_sidecar.cli._dispatch_synthesis", _never)

        args = MockArgs(text="hola", label="saludo", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_STATE_CONFLICT
        assert not dispatch_called[0]

    def test_force_on_free_label_persists(self, tmp_path, monkeypatch):
        """--force con etiqueta libre persiste la toma."""
        store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        args = MockArgs(text="hola", label="nueva", voice="default", force=True)
        cmd_speech_synthesize(args)
        assert synthetic_speech.exists("default", "nueva")

    def test_illegal_label_exits_2(self, tmp_path, monkeypatch):
        """Identificador ilegal (etiqueta) → exit 2."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._validate_path_segment",
                            MagicMock(side_effect=ValueError("ilegal")))
        args = MockArgs(text="hola", label="..", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_voice_not_found_exits_3(self, tmp_path, monkeypatch):
        """Voz inexistente → exit 3."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value=None))
        args = MockArgs(text="hola", label="saludo", voice="inexistente")
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_NOT_FOUND

    def test_model_missing_exits_4(self, tmp_path, monkeypatch):
        """Modelo no provisionado → exit 4."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_MODEL_MISSING

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=False))
        args = MockArgs(text="hola", label="saludo")
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_MODEL_MISSING

    def test_json_plus_play_exits_2(self, tmp_path, monkeypatch):
        """Regla 2 (§2.6): --json y --play → exit 2."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        args = MockArgs(text="hola", label="saludo", voice="default", json=True, play=True)
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_play_without_tty_exits_2(self, tmp_path, monkeypatch):
        """Regla 5 (§2.6): --play sin terminal → exit 2."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr("sys.stdin.isatty", lambda: False)
        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_illegal_voice_exits_2(self, tmp_path, monkeypatch):
        """Identificador ilegal (voz) → exit 2."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._validate_path_segment",
                            MagicMock(side_effect=ValueError("ilegal")))
        args = MockArgs(text="hola", label="saludo", voice="..")
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_INVALID_INPUT


class TestCmdSynthesizePlayLoop:
    """Bucle interactivo de `speech synthesize --play` (§2.4): una fila
    por cada opción del menú 1–4 y cada caso de borde."""

    def _fake_env(self, tmp_path, monkeypatch):
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        # --play exige terminal interactiva: isatty debe retornar True.
        monkeypatch.setattr("sys.stdin.isatty", lambda: True)

    def test_option_4_discards_leaving_no_wav(self, tmp_path, monkeypatch):
        """Opción 4: rechazar y descartar → no queda WAV ni sidecar, exit 0."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=["4"]))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        cmd_speech_synthesize(args)

        from tts_sidecar import synthetic_speech
        assert not synthetic_speech.exists("default", "saludo")

    def test_option_2_accepts_and_persists(self, tmp_path, monkeypatch):
        """Opción 2: aceptar y guardar → persiste la toma, exit 0."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=["2"]))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        cmd_speech_synthesize(args)

        from tts_sidecar import synthetic_speech
        assert synthetic_speech.exists("default", "saludo")

    def test_option_1_then_2_replays_without_resynthesizing(self, tmp_path, monkeypatch):
        """Opción 1 (repetir) luego 2 (aceptar): _dispatch_synthesis llamado
        una sola vez (cero re-síntesis al repetir)."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        dispatch_count = [0]
        def _counted_dispatch(*_, **__):
            dispatch_count[0] += 1
            return (_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)

        monkeypatch.setattr("tts_sidecar.cli._dispatch_synthesis", _counted_dispatch)
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=["1", "2"]))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        cmd_speech_synthesize(args)

        assert dispatch_count[0] == 1

    def test_option_3_then_4_resynthesizes_discards(self, tmp_path, monkeypatch):
        """Opción 3 (regenerar) luego 4 (descartar): dos síntesis, nada persistido."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        dispatch_count = [0]
        def _counted_dispatch(*_, **__):
            dispatch_count[0] += 1
            return (_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)

        monkeypatch.setattr("tts_sidecar.cli._dispatch_synthesis", _counted_dispatch)
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=["3", "4"]))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        cmd_speech_synthesize(args)

        assert dispatch_count[0] == 2
        assert not synthetic_speech.exists("default", "saludo")

    def test_eof_discards(self, tmp_path, monkeypatch):
        """Ctrl-D (EOFError) en el bucle → descarta, no persiste."""
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar import synthetic_speech
        from unittest.mock import MagicMock

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)),
        )
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=EOFError))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        cmd_speech_synthesize(args)

        assert not synthetic_speech.exists("default", "saludo")

    def test_collision_on_accept_exits_6(self, tmp_path, monkeypatch):
        """Cuando se acepta (opción 2) y la etiqueta quedó ocupada sin --force → exit 6."""
        store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_STATE_CONFLICT

        # La colisión ocurre al revalidar en la aceptación (opción 2).
        # Para lograrlo: la primera vez (antes de sintetizar) la etiqueta no existe;
        # pero al revalidar en la aceptación, sí existe.
        synthetic_speech.save("default", "otra", b"RIFFold", "otra locución")

        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        call_count = [0]
        original_exists = synthetic_speech.exists

        def _exists_with_collision(v, l):
            call_count[0] += 1
            if l == "saludo":
                # La etiqueta "saludo" aparece como existente solo en la revalidación (llamada 2+).
                return call_count[0] >= 2
            return original_exists(v, l)

        monkeypatch.setattr(synthetic_speech, "exists", _exists_with_collision)

        monkeypatch.setattr(
            "tts_sidecar.cli._dispatch_synthesis",
            MagicMock(return_value=(_synth_result(b"RIFFdata", t3=1.0, s3gen=2.0), False)),
        )
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(side_effect=["2"]))

        args = MockArgs(text="hola", label="saludo", voice="default", play=True)
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_STATE_CONFLICT


class TestCmdSpeechPlay:
    """Matriz de `speech play` (§2.7, §2.10)."""

    def _fake_env(self, tmp_path, monkeypatch):
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        from tts_sidecar import synthetic_speech
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")

    def test_play_existing_reproduces_and_emits_json(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_play
        from unittest.mock import MagicMock

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        emitted = {}
        monkeypatch.setattr("tts_sidecar.cli.emit_json", lambda p: emitted.update(p))

        args = MockArgs(label="saludo", voice="default", json=True)
        cmd_speech_play(args)

        assert emitted == {"voice": "default", "label": "saludo"}

    def test_play_missing_label_exits_3(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_play
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        args = MockArgs(label="ausente", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_play(args)
        assert exc.value.code == EXIT_NOT_FOUND

    def test_play_illegal_label_exits_2(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_play
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        args = MockArgs(label="..", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_play(args)
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_play_voice_not_found_exits_3(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_play
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value=None))
        args = MockArgs(label="saludo", voice="inexistente")
        with pytest.raises(CliError) as exc:
            cmd_speech_play(args)
        assert exc.value.code == EXIT_NOT_FOUND


class TestCmdSpeechList:
    """Matriz de `speech list` (§2.7)."""

    def _fake_env(self, tmp_path, monkeypatch):
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        from tts_sidecar import synthetic_speech
        synthetic_speech.save("default", "saludo", b"RIFFa", "Hola mundo")
        synthetic_speech.save("otra", "despedida", b"RIFFb", "Adiós")

    def test_list_all_enumerates_by_wav(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_list
        from unittest.mock import patch

        with patch("tts_sidecar.voices._resolve_voice_dir", return_value="/fake/default"), \
             patch("tts_sidecar.voices._resolve_voice_dir", return_value="/fake/otra"):
            # Solo verifica que la función no lanza excepción con múltiples voces.
            args = MockArgs()
            # Sin --json; la función imprime por stdout (no raise).
            cmd_speech_list(args)

    def test_list_filters_by_voice(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_list
        from unittest.mock import MagicMock

        emitted = {}
        monkeypatch.setattr("tts_sidecar.cli.emit_json", lambda p: emitted.update(p))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/otra"))

        args = MockArgs(voice="otra", json=True)
        cmd_speech_list(args)

        assert len(emitted["synthetic_speech"]) == 1
        entry = emitted["synthetic_speech"][0]
        assert set(entry.keys()) == {"voice", "label", "text", "created_at"}
        assert entry["voice"] == "otra"
        assert entry["label"] == "despedida"
        assert entry["text"] == "Adiós"
        assert entry["created_at"] is not None

    def test_list_tolerates_orphan_sidecar(self, tmp_path, monkeypatch):
        """El listado enumera por WAV y tolera sidecar ausente (text=None)."""
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        from tts_sidecar import synthetic_speech
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        # Elimino el sidecar: queda el WAV huérfano.
        wav = synthetic_speech.wav_path("default", "saludo")
        os.unlink(wav.replace(".wav", ".json"))

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        entries = synthetic_speech.list_entries(voice="default")
        assert len(entries) == 1
        assert entries[0]["text"] is None
        assert entries[0]["created_at"] is None

    def test_list_voice_not_found_exits_3(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_list
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value=None))
        args = MockArgs(voice="inexistente")
        with pytest.raises(CliError) as exc:
            cmd_speech_list(args)
        assert exc.value.code == EXIT_NOT_FOUND

    def test_list_orphan_wav_tolerates_missing_sidecar_json(self, tmp_path, monkeypatch, capsys):
        """El CLI lista orfanos de sidecar (WAV sin JSON) con text=None,
        created_at=None, sin fallar."""
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        from tts_sidecar import synthetic_speech
        from tts_sidecar.cli import cmd_speech_list
        import json

        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")
        # Borro el JSON: queda el WAV huérfano sin metadato.
        sidecar = synthetic_speech.wav_path("default", "saludo")[: -len(".wav")] + ".json"
        os.unlink(sidecar)

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        args = MockArgs(json=True)
        cmd_speech_list(args)

        payload = json.loads(capsys.readouterr().out)
        entries = payload["synthetic_speech"]
        assert len(entries) == 1
        assert entries[0]["voice"] == "default"
        assert entries[0]["label"] == "saludo"
        assert entries[0]["text"] is None
        assert entries[0]["created_at"] is None


class TestCmdSpeechRemove:
    """Matriz de `speech remove` (§2.7, §2.10)."""

    def _fake_env(self, tmp_path, monkeypatch):
        store = tmp_path / "synthetic-speech"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))
        from tts_sidecar import synthetic_speech
        synthetic_speech.save("default", "saludo", b"RIFFdata", "Hola")

    def test_remove_existing_deletes_wav_and_sidecar(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_remove
        from unittest.mock import MagicMock

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        emitted = {}
        monkeypatch.setattr("tts_sidecar.cli.emit_json", lambda p: emitted.update(p))

        args = MockArgs(label="saludo", voice="default", json=True)
        cmd_speech_remove(args)

        assert emitted == {"voice": "default", "label": "saludo"}

    def test_remove_missing_label_exits_3(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_remove
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        args = MockArgs(label="ausente", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_remove(args)
        assert exc.value.code == EXIT_NOT_FOUND

    def test_remove_illegal_label_exits_2(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_remove
        from tts_sidecar.exit_codes import EXIT_INVALID_INPUT

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        args = MockArgs(label="..", voice="default")
        with pytest.raises(CliError) as exc:
            cmd_speech_remove(args)
        assert exc.value.code == EXIT_INVALID_INPUT

    def test_remove_voice_not_found_exits_3(self, tmp_path, monkeypatch):
        self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_speech_remove
        from tts_sidecar.exit_codes import EXIT_NOT_FOUND

        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value=None))
        args = MockArgs(label="saludo", voice="inexistente")
        with pytest.raises(CliError) as exc:
            cmd_speech_remove(args)
        assert exc.value.code == EXIT_NOT_FOUND

class TestCmdSpeechDaemonDispatch:
    """Tres modos de despacho (§2.5) para synthesize y clone."""

    def test_explicit_daemon_down_exits_5_synthesize(self, tmp_path, monkeypatch):
        """--daemon sin daemon activo → exit 5 (_dispatch_synthesis comprueba
        is_daemon_running y aborta antes de sintetizar)."""
        from tts_sidecar.cli import cmd_speech_synthesize
        from tts_sidecar.exit_codes import EXIT_DAEMON_UNREACHABLE

        store = tmp_path / "store"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=False))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        args = MockArgs(text="hola", label="daemon-down-label", voice="default", daemon=True)
        with pytest.raises(CliError) as exc:
            cmd_speech_synthesize(args)
        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    def test_explicit_daemon_down_exits_5_clone(self, tmp_path, monkeypatch):
        """--daemon sin daemon activo en voice clone → exit 5."""
        from tts_sidecar.cli import cmd_voice_clone
        from tts_sidecar.exit_codes import EXIT_DAEMON_UNREACHABLE

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=False))
        monkeypatch.setattr("tts_sidecar.voices.clone_voice_files",
                            MagicMock(return_value=(tmp_path / "t.wav", tmp_path / "s.wav")))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))

        args = MockArgs(name="nueva", timbre_reference=str(tmp_path / "t.wav"),
                        speech_reference=str(tmp_path / "s.wav"), daemon=True)
        with pytest.raises(CliError) as exc:
            cmd_voice_clone(args)
        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    def test_explicit_daemon_down_exits_5_clone_without_timbre(self, tmp_path, monkeypatch):
        """--daemon sin daemon activo en voice clone sin timbre → exit 5 también
        (el despacho no depende de si hay timbre)."""
        from tts_sidecar.cli import cmd_voice_clone
        from tts_sidecar.exit_codes import EXIT_DAEMON_UNREACHABLE

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=False))
        monkeypatch.setattr("tts_sidecar.voices.clone_voice_files",
                            MagicMock(return_value=(None, tmp_path / "s.wav")))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))

        args = MockArgs(name="nueva", timbre_reference=None,
                        speech_reference=str(tmp_path / "s.wav"), daemon=True)
        with pytest.raises(CliError) as exc:
            cmd_voice_clone(args)
        assert exc.value.code == EXIT_DAEMON_UNREACHABLE

    def test_explicit_no_daemon_forces_direct_no_synth_called(self, tmp_path, monkeypatch):
        """--no-daemon fuerza ruta directa sin sondear daemon."""
        from tts_sidecar.cli import cmd_voice_clone

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=False))
        monkeypatch.setattr("tts_sidecar.voices.clone_voice_files",
                            MagicMock(return_value=(tmp_path / "t.wav", tmp_path / "s.wav")))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(tmp_path / "voices"))
        mock_engine_cls = MagicMock()
        mock_engine_cls.get_instance.return_value.precompute_voice.return_value = True
        monkeypatch.setattr("tts_sidecar.engine.ChatterboxEngine", mock_engine_cls)

        args = MockArgs(name="nueva", timbre_reference=str(tmp_path / "t.wav"),
                        speech_reference=str(tmp_path / "s.wav"), no_daemon=True)
        cmd_voice_clone(args)
        # Llegar aquí sin CliError de daemon inalcanzable = test pasó.

    def test_autodetect_with_daemon_running_uses_daemon_for_synthesize(
        self, tmp_path, monkeypatch
    ):
        """Sin flags, daemon corriendo → autodetección usa daemon (daemon_flag=True)."""
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        store = tmp_path / "store"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.cli._dispatch_synthesis",
                            MagicMock(return_value=(_synth_result(b"RIFF", t3=0.0, s3gen=0.0), True)))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))

        args = MockArgs(text="hola", label="autodetect-label", voice="default")
        cmd_speech_synthesize(args)
        # El resultado del dispatch tiene daemon_flag=True; main() no lanza excepción = éxito.

    def test_autodetect_daemon_down_falls_back_to_direct(self, tmp_path, monkeypatch):
        """Sin flags, daemon no responde → autodetección degrada a modo
        directo y completa con éxito (no sale 5)."""
        from tts_sidecar.cli import cmd_speech_synthesize
        from unittest.mock import MagicMock

        store = tmp_path / "store"
        store.mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))

        monkeypatch.setattr("tts_sidecar.daemon.is_daemon_running", MagicMock(return_value=False))
        monkeypatch.setattr("tts_sidecar.cli._dispatch_synthesis",
                            MagicMock(return_value=(_synth_result(), False)))
        monkeypatch.setattr("tts_sidecar.model_cache.is_model_cached", MagicMock(return_value=True))
        monkeypatch.setattr("tts_sidecar.voices._resolve_voice_dir", MagicMock(return_value="/fake/default"))
        monkeypatch.setattr("sys.stdin.isatty", lambda: True)
        monkeypatch.setattr("tts_sidecar.cli._play_audio", MagicMock())
        monkeypatch.setattr("builtins.input", MagicMock(return_value="4"))

        args = MockArgs(text="hola", label="fallback-label", voice="default", play=True)
        cmd_speech_synthesize(args)
        # Llegar aquí sin CliError = fallback directo exitoso.

class TestCmdCleanupSyntheticSpeech:
    """Tests de `cleanup --synthetic-speech`, `--voices` drag y `--all`
    incluyendo el almacén (§2.11)."""

    def _args(self, **kw):
        return MockArgs(
            model=kw.get("model", False),
            voices=kw.get("voices", False),
            synthetic_speech=kw.get("synthetic_speech", False),
            all=kw.get("all", False),
            dry_run=kw.get("dry_run", False),
            yes=kw.get("yes", False),
            json=kw.get("json", False),
            cleanup_parser=MagicMock(),
        )

    def _fake_env(self, tmp_path, monkeypatch):
        """Caché HF sintética + modelo + voces + almacén aislado con 'default/'."""
        hub = tmp_path / "hub"
        propio1 = hub / "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
        propio2 = hub / "models--ResembleAI--chatterbox"
        for d in (propio1, propio2):
            d.mkdir(parents=True)
        from huggingface_hub import constants
        monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))

        voices = tmp_path / "voices"
        (voices / "mi_voz").mkdir(parents=True)
        monkeypatch.setattr("tts_sidecar.voices.voices_root", lambda: str(voices))

        store = tmp_path / "synthetic-speech"
        store.mkdir()
        (store / "default").mkdir()
        (store / "mi_voz").mkdir()
        monkeypatch.setattr("tts_sidecar.synthetic_speech.store_root", lambda: str(store))

        return propio1, propio2, voices, store

    def test_cleanup_synthetic_speech_deletes_store_root(self, tmp_path, monkeypatch):
        """--synthetic-speech borra la raíz del almacén entera."""
        _, _, _, store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_cleanup

        cmd_cleanup(self._args(synthetic_speech=True, yes=True))
        assert not store.exists()

    def test_cleanup_voices_drag_excludes_default_namespace(self, tmp_path, monkeypatch):
        """--voices arrastra los namespaces de habla sintética de las voces
        borradas, pero preserva el namespace 'default' (voz de fábrica)."""
        _, _, _, store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_cleanup

        # Evitar que se intente borrar dirs del modelo real que existen en este equipo.
        monkeypatch.setattr(
            "tts_sidecar.model_cache.model_cache_dirs",
            MagicMock(return_value=[]),
        )
        cmd_cleanup(self._args(voices=True, yes=True))
        # El namespace 'default' del almacén debe sobrevivir a --voices.
        assert (store / "default").exists(), "default sobrevive a --voices"

    def test_cleanup_all_includes_store(self, tmp_path, monkeypatch):
        """--all incluye el almacén de habla sintética en el borrado."""
        _, _, _, store = self._fake_env(tmp_path, monkeypatch)
        monkeypatch.setattr(
            "tts_sidecar.model_cache.model_cache_dirs",
            MagicMock(return_value=[]),
        )
        from tts_sidecar.cli import cmd_cleanup

        cmd_cleanup(self._args(all=True, yes=True))
        # El almacén entero se borra con --all.
        assert not store.exists()

    def test_cleanup_dry_run_does_not_delete(self, tmp_path, monkeypatch, capsys):
        """--dry-run enumera las rutas sin borrarlas."""
        _, _, _, store = self._fake_env(tmp_path, monkeypatch)
        from tts_sidecar.cli import cmd_cleanup

        cmd_cleanup(self._args(synthetic_speech=True, dry_run=True))
        assert store.exists(), "dry-run no borró nada"
        assert "dry-run" in capsys.readouterr().out
