"""
Utilidades de temporización compartidas para ai-voice-interconnector.

Todo el flujo de progreso e instrumentación (log, StageTimer, los banners de
timed_command) se emite a **stderr**: stdout queda reservado para datos
(salidas --json, listados, confirmaciones de resultado), cumpliendo el contrato
de salida del CLI.
"""

import contextvars
import sys
import threading
import time
from dataclasses import dataclass
from datetime import datetime
from functools import wraps
from typing import Optional


@dataclass
class SynthesisMetrics:
    """Tiempos por sub-etapa de una síntesis (segundos).

    Value object tipado que reemplaza el dict suelto `{'t3': .., 's3gen': ..}`
    que el engine exponía y el daemon leía vía `getattr(engine, ...)`. Modelar
    las métricas de forma explícita rompe ese acoplamiento implícito: el engine
    publica un `SynthesisMetrics` y el daemon lo consume como un contrato tipado
    (campos `t3`/`s3gen`) en vez de un dict con claves por convención.
    """
    t3: float = 0.0
    s3gen: float = 0.0


@dataclass
class SynthesisResult:
    """Resultado de una síntesis: audio + métricas, en un único objeto.

    Reemplaza el retorno `bytes` desnudo de `engine.synthesize()` /
    `SynthesisOrchestrator.synthesize()` / `DaemonIPCClient.synthesize()`: antes
    las métricas (`SynthesisMetrics`) solo existían como texto de log a stderr y
    el llamador no tenía forma programática de leerlas. Con este objeto, ambas
    rutas de síntesis (directa y vía daemon) devuelven la misma forma por
    construcción, sin estado mutable ni orden temporal frágil entre leer el
    audio y leer las métricas.
    """
    audio_bytes: bytes
    metrics: SynthesisMetrics


# Spinner activo por contexto de ejecución (a lo sumo uno). log() lo consulta
# para no entremezclar sus líneas con el redibujado del spinner; ver Spinner.
# Es un ContextVar, no un global mutable de módulo: el estado del spinner queda
# aislado por contexto/hilo y es testeable sin ensuciar estado compartido entre
# pruebas ni acoplar log() a una variable de módulo reasignable.
_active_spinner: "contextvars.ContextVar[Optional[Spinner]]" = contextvars.ContextVar(
    "_active_spinner", default=None
)
# Serializa la escritura al stream entre el hilo de redibujado del Spinner y
# write_line(): protege el I/O del stream compartido, no la referencia al spinner.
_spinner_lock = threading.Lock()


def log(msg: str, duration: Optional[float] = None):
    """Imprime un mensaje de log con formato consistente (a stderr).

    Sin duration: [HH:MM:SS] Mensaje...
    Con duration: [HH:MM:SS] Mensaje -> Done (Xs)

    Si hay un Spinner activo, la línea se emite de forma coordinada: el spinner
    limpia su renglón antes de imprimir y lo redibuja después, para que ambos no
    colisionen en el mismo stderr.
    """
    now = datetime.now().strftime("%H:%M:%S")
    if duration is not None:
        line = f"[{now}] {msg} -> Done ({duration:.1f}s)"
    else:
        line = f"[{now}] {msg}..."

    spinner = _active_spinner.get()
    if spinner is not None:
        spinner.write_line(line)
    else:
        print(line, file=sys.stderr)


def format_progress_event(ev: dict) -> str:
    """Formatea un evento de progreso del motor a una etiqueta humana única.

    Fuente única evento→etiqueta compartida por el modo daemon (eventos que
    llegan por el stream NDJSON) y el modo directo (progress_callback del motor),
    de modo que ambos actualicen el Spinner con el mismo texto. Incluye el
    conteo de tokens del T3 cuando está presente (p. ej. «Generando voz · 210
    tokens»).
    """
    stage = ev.get("stage")
    tokens = ev.get("tokens")

    if stage == "t3":
        if tokens:
            return f"Generando voz · {tokens} tokens"
        return "Generando voz…"

    labels = {
        "conditionals": "Preparando la voz…",
        "tts": "Generando voz…",
        "s3gen": "Sintetizando audio (vocoder)…",
        "encoding": "Codificando audio…",
        "saving": "Guardando…",
    }
    return labels.get(stage, "Sintetizando…")


def timed_command(func):
    """Decorador que añade información de tiempo a funciones de comando CLI.

    Registra el inicio y el fin del comando (a stderr). Los timestamps los
    maneja StageTimer/log para evitar duplicación.
    """
    @wraps(func)
    def wrapper(args):
        start_time = time.time()
        print(f"Iniciando {func.__name__.replace('cmd_', '')}...", file=sys.stderr)
        try:
            result = func(args)
            elapsed = time.time() - start_time
            print(f"Finalizado en {elapsed:.1f}s", file=sys.stderr)
            return result
        except Exception as e:
            elapsed = time.time() - start_time
            print(f"Falló tras {elapsed:.1f}s: {e}", file=sys.stderr)
            raise
    return wrapper


def timed(stage_name: str):
    """Decorador que registra el tiempo de ejecución de una función como etapa.

    Usa log() para imprimir la duración al terminar la función.
    """
    def decorator(func):
        @wraps(func)
        def wrapper(*args, **kwargs):
            start = time.time()
            result = func(*args, **kwargs)
            duration = time.time() - start
            log(f"[{stage_name}]", duration)
            return result
        return wrapper
    return decorator


class StageTimer:
    """Context manager para temporizar un bloque de código.

    Imprime el inicio con log() al entrar y la duración al salir.

    Uso:
        with StageTimer("MiEtapa", "Descripción"):
            # código a temporizar
    """

    def __init__(self, name: str, description: str = None, spinner: bool = False):
        self.name = name
        self.description = description or name
        self.start = None
        # Si spinner=True, muestra un Spinner de liveness (solo en TTY) mientras
        # dura el bloque; las líneas de log() internas se coordinan con él.
        self._spinner = Spinner(self.description) if spinner else None

    def __enter__(self):
        self.start = time.time()
        log(f"[{self.name}] {self.description}...")
        if self._spinner is not None:
            self._spinner.__enter__()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if self._spinner is not None:
            self._spinner.__exit__(exc_type, exc_val, exc_tb)
        duration = time.time() - self.start
        log(f"[{self.name}]", duration)
        return False


class Spinner:
    """Indicador de liveness (spinner + tiempo transcurrido) sobre stderr.

    Un hilo de fondo redibuja en el sitio (con `\\r`) una línea del tipo
    `⠋ Sintetizando voz… (12.3s)` mientras dura el bloque `with`. Su único
    propósito es demostrar que el proceso no está colgado durante operaciones
    largas y opacas (carga del modelo, síntesis, espera del daemon).

    Diseño:
    - **Solo TTY**: si stderr no es una terminal interactiva (piped, daemon, CI,
      tests) es un no-op total; no escribe nada y preserva el contrato del CLI
      (stdout = datos; stderr = progreso legible por humanos).
    - **stderr únicamente**: nunca toca stdout.
    - **Coordinado con log()**: mientras está activo se registra en el global
      `_active_spinner`; log() usa write_line() para intercalar sus líneas sin
      pisar el renglón del spinner.
    - **API para Fase 2**: update(text) permite cambiar la etiqueta en caliente
      (p. ej. progreso real de tokens transmitido por el daemon) sin refactor.
    """

    # Frames braille (suaves) y su alternativa ASCII para consolas sin UTF-8.
    _FRAMES_UNICODE = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
    _FRAMES_ASCII = "|/-\\"
    _INTERVAL = 0.1  # segundos entre frames

    def __init__(self, label: str, stream=None):
        self._label = label
        self._stream = stream if stream is not None else sys.stderr
        self._enabled = self._stream_is_tty()
        self._frames = self._pick_frames()
        self._start = None
        self._thread = None
        self._stop = threading.Event()
        self._lock = threading.Lock()
        # Token de reset del ContextVar _active_spinner, fijado en __enter__.
        self._token = None

    def _stream_is_tty(self) -> bool:
        try:
            return bool(self._stream.isatty())
        except Exception:
            # Silencio deliberado: este guarda protege el propio I/O de
            # stderr; loguear aquí escribiría al mismo stream que acaba de fallar.
            # Ante la duda, se asume «no es TTY» (el Spinner queda inerte).
            return False

    def _pick_frames(self) -> str:
        """Braille si el encoding del stream soporta UTF-8; ASCII en su defecto."""
        enc = (getattr(self._stream, "encoding", None) or "").lower()
        if "utf" in enc:
            try:
                self._FRAMES_UNICODE.encode(self._stream.encoding)
                return self._FRAMES_UNICODE
            except (UnicodeEncodeError, LookupError, TypeError):
                pass
        return self._FRAMES_ASCII

    def update(self, label: str) -> None:
        """Cambia la etiqueta mostrada (thread-safe)."""
        with self._lock:
            self._label = label

    def _render(self, frame: str) -> str:
        with self._lock:
            label = self._label
        elapsed = time.time() - self._start if self._start else 0.0
        return f"{frame} {label} ({elapsed:.1f}s)"

    def _run(self) -> None:
        i = 0
        while not self._stop.is_set():
            frame = self._frames[i % len(self._frames)]
            line = self._render(frame)
            with _spinner_lock:
                # \r al inicio + \x1b[K borra hasta el fin de línea, evitando
                # residuos si la etiqueta se acorta entre frames.
                self._stream.write("\r\x1b[K" + line)
                self._stream.flush()
            i += 1
            self._stop.wait(self._INTERVAL)

    def _clear_line(self) -> None:
        try:
            self._stream.write("\r\x1b[K")
            self._stream.flush()
        except Exception:
            # Silencio deliberado: igual que _stream_is_tty, un fallo al
            # escribir en stderr no puede reportarse por stderr; se descarta.
            pass

    def write_line(self, text: str) -> None:
        """Emite una línea de log coordinada con el spinner.

        Limpia el renglón del spinner, imprime `text` con salto de línea y deja
        que el hilo lo redibuje en el siguiente frame. Si el spinner está
        desactivado (no TTY), simplemente imprime a stderr como haría log().
        """
        if not self._enabled:
            print(text, file=self._stream)
            return
        with _spinner_lock:
            self._stream.write("\r\x1b[K" + text + "\n")
            self._stream.flush()

    def __enter__(self):
        if not self._enabled:
            return self
        self._start = time.time()
        self._stop.clear()
        # set() devuelve un token que reset() consume en __exit__, restaurando
        # el valor previo del contexto (soporta anidamiento correctamente).
        self._token = _active_spinner.set(self)
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if not self._enabled:
            return False
        self._stop.set()
        if self._thread is not None:
            self._thread.join(timeout=1.0)
        with _spinner_lock:
            self._clear_line()
        _active_spinner.reset(self._token)
        self._token = None
        return False  # nunca traga excepciones
