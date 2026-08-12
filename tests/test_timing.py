"""Tests para las utilidades de timing.py.

Todo el flujo de progreso (log, StageTimer, banners de
timed_command) se emite a stderr; stdout queda reservado para datos.
"""

import pytest
import sys
import time
from pathlib import Path
from io import StringIO

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))
from ai_voice_interconnector import timing
from ai_voice_interconnector.timing import (
    log, timed_command, timed, StageTimer, Spinner, format_progress_event,
)


class TestFormatProgressEvent:
    def test_t3_with_tokens(self):
        assert format_progress_event(
            {"event": "progress", "stage": "t3", "tokens": 210}
        ) == "Generando voz · 210 tokens"

    def test_t3_without_tokens(self):
        assert format_progress_event({"event": "progress", "stage": "t3"}) == "Generando voz…"

    def test_known_stages(self):
        assert format_progress_event({"stage": "conditionals"}) == "Preparando la voz…"
        assert format_progress_event({"stage": "s3gen"}).startswith("Sintetizando audio")
        assert format_progress_event({"stage": "encoding"}) == "Codificando audio…"
        assert format_progress_event({"stage": "saving"}) == "Guardando…"

    def test_unknown_stage_falls_back_to_default(self):
        assert format_progress_event({"stage": "otra"}) == "Sintetizando…"
        assert format_progress_event({}) == "Sintetizando…"


class FakeTTY(StringIO):
    """Stream de texto en memoria que se hace pasar por terminal interactiva."""

    def __init__(self, encoding="utf-8"):
        super().__init__()
        self._encoding = encoding

    def isatty(self):
        return True

    @property
    def encoding(self):
        return self._encoding


class TestLog:
    def test_log_without_duration(self, capsys):
        log("test message")
        captured = capsys.readouterr()
        assert "test message..." in captured.err
        assert captured.out == ""

    def test_log_with_duration(self, capsys):
        log("test operation", duration=1.5)
        captured = capsys.readouterr()
        assert "test operation" in captured.err
        assert "Done (1.5s)" in captured.err
        assert captured.out == ""


class TestTimedCommand:
    def test_timed_command_start_finish(self, capsys):
        @timed_command
        def dummy_cmd(args):
            print("inside command")
            return 42

        class Args:
            pass

        result = dummy_cmd(Args())
        captured = capsys.readouterr()
        assert "Iniciando dummy_cmd..." in captured.err
        assert "Finalizado en" in captured.err
        # Los datos del comando siguen yendo a stdout
        assert "inside command" in captured.out
        assert result == 42

    def test_timed_command_error(self, capsys):
        @timed_command
        def failing_cmd(args):
            raise ValueError("test error")

        class Args:
            pass

        with pytest.raises(ValueError):
            failing_cmd(Args())
        captured = capsys.readouterr()
        assert "Falló tras" in captured.err
        assert "test error" in captured.err
        assert captured.out == ""


class TestTimedDecorator:
    def test_timed_decorator(self, capsys):
        @timed("MyStage")
        def some_work():
            print("working")

        some_work()
        captured = capsys.readouterr()
        assert "[MyStage]" in captured.err
        assert "working" in captured.out

    def test_preserves_positional_args(self, capsys):
        @timed("Sum")
        def add(a, b):
            return a + b

        result = add(2, 3)
        assert result == 5
        assert "[Sum]" in capsys.readouterr().err

    def test_preserves_keyword_args(self, capsys):
        @timed("Greet")
        def greet(name, greeting="Hola"):
            return f"{greeting}, {name}!"

        result = greet(name="Mundo", greeting="Hola")
        assert result == "Hola, Mundo!"
        assert "[Greet]" in capsys.readouterr().err

    def test_mixed_args_and_kwargs(self, capsys):
        @timed("Mixed")
        def build(a, b, *, c=None):
            return (a, b, c)

        result = build(1, 2, c=3)
        assert result == (1, 2, 3)
        assert "[Mixed]" in capsys.readouterr().err

    def test_return_value_of_none_still_returns_none(self, capsys):
        @timed("NoReturn")
        def no_return():
            pass

        assert no_return() is None
        assert "[NoReturn]" in capsys.readouterr().err

    def test_wraps_preserves_function_metadata(self):
        @timed("Meta")
        def documented():
            """docstring de prueba."""
            pass

        assert documented.__name__ == "documented"
        assert documented.__doc__ == "docstring de prueba."

    def test_exception_propagates_without_logging_stage(self, capsys):
        """timed() no envuelve la llamada en try/except: una excepción se
        propaga tal cual y el log de duración (posterior a func()) no llega
        a emitirse, a diferencia de timed_command que sí registra el fallo."""
        @timed("Failing")
        def failing():
            raise ValueError("boom")

        with pytest.raises(ValueError, match="boom"):
            failing()
        assert "[Failing]" not in capsys.readouterr().err

    def test_measures_nonzero_duration_for_slow_call(self, capsys):
        @timed("Slow")
        def slow():
            time.sleep(0.05)

        slow()
        captured = capsys.readouterr()
        assert "[Slow]" in captured.err
        assert "Done (" in captured.err


class TestStageTimer:
    def test_stage_timer_entry_exit(self, capsys):
        with StageTimer("TestStage", "doing work"):
            print("inside stage")
        captured = capsys.readouterr()
        assert "[TestStage] doing work..." in captured.err
        assert "Done (" in captured.err
        assert "inside stage" in captured.out

    def test_stage_timer_no_description(self, capsys):
        with StageTimer("X"):
            pass
        assert "[X] X..." in capsys.readouterr().err

    def test_stage_timer_exception(self, capsys):
        with pytest.raises(RuntimeError):
            with StageTimer("Fail"):
                raise RuntimeError("boom")
        # Debe registrar el fin de la etapa incluso cuando hay excepción
        assert "[Fail]" in capsys.readouterr().err


class TestSpinner:
    def test_noop_when_not_tty(self, capsys):
        # stderr bajo capsys no es TTY: el spinner debe ser un no-op total.
        with Spinner("trabajando") as sp:
            assert sp._enabled is False
            time.sleep(0.05)
        captured = capsys.readouterr()
        assert captured.out == ""
        # No deja el spinner activo en el contexto ni caracteres de control.
        assert timing._active_spinner.get() is None

    def test_update_headless_does_not_fail(self):
        with Spinner("a") as sp:
            sp.update("b")  # no debe lanzar aunque esté deshabilitado
        assert True

    def test_active_writes_only_to_its_stream(self):
        stream = FakeTTY()
        with Spinner("sintetizando", stream=stream) as sp:
            assert sp._enabled is True
            assert timing._active_spinner.get() is sp
            time.sleep(0.25)  # al menos un par de frames
        # Escribió frames a su stream; nunca a stdout real.
        out = stream.getvalue()
        assert "sintetizando" in out
        assert "\r" in out  # redibujado en el sitio
        # Tras salir, el contexto queda limpio.
        assert timing._active_spinner.get() is None

    def test_ascii_fallback_for_non_utf8(self):
        stream = FakeTTY(encoding="cp1252")
        sp = Spinner("x", stream=stream)
        # Sin UTF-8 usa frames ASCII, no braille.
        assert sp._frames == Spinner._FRAMES_ASCII

    def test_utf8_uses_braille(self):
        sp = Spinner("x", stream=FakeTTY(encoding="utf-8"))
        assert sp._frames == Spinner._FRAMES_UNICODE

    def test_log_coordinates_with_active_spinner(self):
        stream = FakeTTY()
        with Spinner("trabajo", stream=stream):
            log("evento intermedio")
        out = stream.getvalue()
        # La línea de log se intercaló en el mismo stream del spinner.
        assert "evento intermedio" in out

    def test_render_reflects_update(self):
        sp = Spinner("inicial", stream=FakeTTY())
        sp._start = time.time()
        assert "inicial" in sp._render(sp._frames[0])
        sp.update("nuevo")
        assert "nuevo" in sp._render(sp._frames[0])

    def test_exception_cleans_up_global(self, capsys):
        stream = FakeTTY()
        with pytest.raises(RuntimeError):
            with Spinner("x", stream=stream):
                raise RuntimeError("boom")
        # No debe tragar la excepción ni dejar el spinner registrado.
        assert timing._active_spinner.get() is None

    def test_active_spinner_is_isolated_per_thread(self):
        """El spinner activo vive en un ContextVar, no en un global compartido:
        un spinner activo en el hilo principal no se filtra a otro hilo, que ve
        su propio contexto (default None). Este aislamiento es justo lo que un
        global mutable de módulo no daba y es la razón del refactor."""
        import threading

        seen_in_worker = {}

        def worker():
            seen_in_worker["value"] = timing._active_spinner.get()

        with Spinner("principal", stream=FakeTTY()) as sp:
            assert timing._active_spinner.get() is sp
            t = threading.Thread(target=worker)
            t.start()
            t.join(timeout=5)

        # El hilo worker arranca con un contexto propio: no ve el spinner del
        # hilo principal (habría sido `sp` con un global de módulo compartido).
        assert seen_in_worker["value"] is None
