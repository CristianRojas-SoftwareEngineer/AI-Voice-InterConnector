"""Tests para tts_sidecar.daemon.run: serve(), signal_handler, atexit y main().

El loop de arranque/reinicio del daemon (--auto-restart/--max-retries),
el registro de manejadores de señal y el registro atexit del pidfile estaban
sin cubrir (solo existían tests para el fallo de bind de puerto y para
_remove_own_pidfile en tests/test_daemon.py). Este archivo mockea el motor y
uvicorn.Server.run para ejercer el flujo de reintentos sin cargar el modelo
real ni abrir un socket real.
"""

import signal
import sys
from pathlib import Path
from unittest.mock import MagicMock, patch

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from tts_sidecar.compute_backend import ComputeBackendResolver
from tts_sidecar.daemon import run as daemon_run
from tts_sidecar.exit_codes import EXIT_STATE_CONFLICT


@pytest.fixture(autouse=True)
def _mock_engine_loading(monkeypatch):
    """Evita cargar el motor real: get_instance/ComputeBackendResolver mockeados."""
    from tts_sidecar.engine import ChatterboxEngine
    from tts_sidecar.compute_backend import ComputeBackendResolver

    monkeypatch.setattr(
        ChatterboxEngine, "get_instance",
        staticmethod(lambda **kw: MagicMock()),
    )
    monkeypatch.setattr(
        ComputeBackendResolver, "resolve",
        staticmethod(lambda compute_backend=None: "cpu"),
    )
    yield


@pytest.fixture(autouse=True)
def _mock_translation_loading(monkeypatch):
    """Evita cargar los modelos de traducción reales en la precarga en caliente
    (Tarea 11, language='en'/'all'): `TranslationModelLoader.load` mockeado para
    no requerir el par opus-mt provisionado en disco durante estos tests."""
    from tts_sidecar.translation import TranslationModelLoader

    monkeypatch.setattr(TranslationModelLoader, "load", lambda self, cache_dir: MagicMock())
    yield


@pytest.fixture(autouse=True)
def _no_real_sleep(monkeypatch):
    """time.sleep(1) entre reintentos no debe ralentizar el suite."""
    monkeypatch.setattr(daemon_run.time, "sleep", MagicMock())
    yield


class TestServeAutoRestart:
    def test_no_auto_restart_runs_once_and_returns(self):
        """Sin --auto-restart, un error genérico en server.run() no reintenta."""
        with patch("uvicorn.Server.run", side_effect=RuntimeError("boom")) as mock_run:
            daemon_run.serve(auto_restart=False)
        assert mock_run.call_count == 1
        daemon_run.time.sleep.assert_not_called()

    def test_generic_exception_retries_until_max(self):
        """Con auto_restart y max_retries=2, se reintenta exactamente 2 veces
        (la compuerta retries>=max_retries se evalúa ANTES de cada intento)."""
        with patch("uvicorn.Server.run", side_effect=RuntimeError("boom")) as mock_run:
            daemon_run.serve(auto_restart=True, max_retries=2)
        assert mock_run.call_count == 2
        assert daemon_run.time.sleep.call_count == 2

    def test_max_retries_gate_prevents_extra_attempt(self):
        """max_retries=1: un único intento, luego la compuerta corta el loop."""
        with patch("uvicorn.Server.run", side_effect=RuntimeError("boom")) as mock_run:
            daemon_run.serve(auto_restart=True, max_retries=1)
        assert mock_run.call_count == 1
        assert daemon_run.time.sleep.call_count == 1

    def test_unbounded_retries_stop_on_keyboard_interrupt(self):
        """max_retries=0 (infinito): sigue reintentando hasta que algo rompe
        el loop; usamos KeyboardInterrupt en el 3er intento para terminarlo
        de forma determinista."""
        with patch(
            "uvicorn.Server.run",
            side_effect=[RuntimeError("boom"), RuntimeError("boom"), KeyboardInterrupt()],
        ) as mock_run:
            daemon_run.serve(auto_restart=True, max_retries=0)
        assert mock_run.call_count == 3
        # KeyboardInterrupt hace un break directo, sin incrementar retries de nuevo.
        assert daemon_run.time.sleep.call_count == 2

    def test_successful_run_without_auto_restart_does_not_retry(self):
        """server.run() retorna normalmente (sin excepción): un solo intento,
        el loop termina porque auto_restart=False."""
        with patch("uvicorn.Server.run", return_value=None) as mock_run:
            daemon_run.serve(auto_restart=False)
        assert mock_run.call_count == 1
        daemon_run.time.sleep.assert_not_called()

    def test_engine_cache_invalidated_between_retries(self):
        """Tras un crash con auto_restart, se invalida la entrada cacheada del
        motor para forzar una recarga real en el siguiente intento."""
        from tts_sidecar.engine import ChatterboxEngine

        cache_key = ComputeBackendResolver.cache_key(model="es-mx-latam", compute_backend="cpu")
        ChatterboxEngine._cache[cache_key] = MagicMock()
        try:
            with patch("uvicorn.Server.run", side_effect=RuntimeError("boom")):
                daemon_run.serve(auto_restart=True, max_retries=1)
            assert cache_key not in ChatterboxEngine._cache
        finally:
            ChatterboxEngine._cache.pop(cache_key, None)

    def test_port_in_use_exits_without_retry_even_with_auto_restart(self):
        """OSError EADDRINUSE debe salir vía sys.exit, sin llegar al camino de
        reintento (el bloque de retry está después del except, nunca se alcanza)."""
        import errno

        with patch(
            "uvicorn.Server.run",
            side_effect=OSError(errno.EADDRINUSE, "Address already in use"),
        ) as mock_run:
            with pytest.raises(SystemExit) as exc:
                daemon_run.serve(auto_restart=True, max_retries=5)
        assert exc.value.code == EXIT_STATE_CONFLICT
        assert mock_run.call_count == 1
        daemon_run.time.sleep.assert_not_called()


class TestServeSignalHandlers:
    def test_registers_sigterm_and_sigint_handlers(self):
        """serve() debe instalar manejadores propios para SIGTERM/SIGINT antes
        de correr el servidor (cubren la ventana previa a uvicorn.Server.run)."""
        with patch("signal.signal") as mock_signal, \
             patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False)

        registered_signals = [call.args[0] for call in mock_signal.call_args_list]
        assert signal.SIGTERM in registered_signals
        assert signal.SIGINT in registered_signals
        # Ambos handlers registrados deben ser invocables y comportarse igual
        # (mismo signal_handler local, cierre vía SystemExit(0)).
        handlers = [call.args[1] for call in mock_signal.call_args_list]
        assert len(handlers) == 2
        for handler in handlers:
            with pytest.raises(SystemExit) as exc:
                handler(signal.SIGTERM, None)
            assert exc.value.code == 0

    def test_signal_handler_logs_before_exiting(self, capsys):
        with patch("signal.signal") as mock_signal, \
             patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False)

        handler = mock_signal.call_args_list[0].args[1]
        with pytest.raises(SystemExit):
            handler(signal.SIGINT, None)
        assert "señal de cierre recibida" in capsys.readouterr().err


class TestServeAtexit:
    def test_registers_remove_own_pidfile_at_exit(self):
        with patch("atexit.register") as mock_atexit, \
             patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False)

        registered_funcs = [call.args[0] for call in mock_atexit.call_args_list]
        assert daemon_run._remove_own_pidfile in registered_funcs

    def test_registers_atexit_exactly_once_per_serve_call_before_loop(self):
        """atexit.register se llama una sola vez por invocación de serve(),
        incluso si el loop reintenta varias veces."""
        with patch("atexit.register") as mock_atexit, \
             patch("uvicorn.Server.run", side_effect=RuntimeError("boom")):
            daemon_run.serve(auto_restart=True, max_retries=3)

        pidfile_registrations = [
            call for call in mock_atexit.call_args_list
            if call.args and call.args[0] is daemon_run._remove_own_pidfile
        ]
        assert len(pidfile_registrations) == 1


class TestServeTranslationPreload:
    """Tarea 11: `serve(language="en"/"all")` precarga en caliente el par de
    traducción opus-mt es<->en antes de servir; `language="es-latam"` no lo toca."""

    def test_language_en_preloads_both_translation_directions(self):
        from tts_sidecar.translation import TranslationModelLoader

        with patch.object(TranslationModelLoader, "load") as mock_load, \
                patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False, language="en")

        loaded_dirs = {call.args[0] for call in mock_load.call_args_list}
        assert len(loaded_dirs) == 2
        from tts_sidecar.daemon.run import app
        assert app.state.daemon.translation_loader is not None
        assert app.state.daemon.translation_service is not None

    def test_language_all_preloads_translation_pair(self):
        from tts_sidecar.translation import TranslationModelLoader

        with patch.object(TranslationModelLoader, "load") as mock_load, \
                patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False, language="all")

        assert mock_load.call_count == 2

    def test_language_es_latam_skips_translation_preload(self):
        from tts_sidecar.translation import TranslationModelLoader
        from tts_sidecar.daemon.run import app

        with patch.object(TranslationModelLoader, "load") as mock_load, \
                patch("uvicorn.Server.run", return_value=None):
            daemon_run.serve(auto_restart=False, language="es-latam")

        mock_load.assert_not_called()
        assert app.state.daemon.translation_loader is None


class TestMain:
    def test_main_delegates_to_serve_with_parsed_args(self):
        with patch.object(sys, "argv", ["daemon-run", "--auto-restart", "--max-retries", "3"]), \
             patch("tts_sidecar.daemon.run.serve") as mock_serve:
            daemon_run.main()
        mock_serve.assert_called_once_with(auto_restart=True, max_retries=3, language="all")

    def test_main_defaults_no_auto_restart_and_zero_retries(self):
        with patch.object(sys, "argv", ["daemon-run"]), \
             patch("tts_sidecar.daemon.run.serve") as mock_serve:
            daemon_run.main()
        mock_serve.assert_called_once_with(auto_restart=False, max_retries=0, language="all")
