"""
Gestor del ciclo de vida del daemon de tts-sidecar.
Maneja los comandos start/stop/restart/status.
"""

import logging
import os
import platform
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Optional

import requests

from .. import paths
from .ipc import DEFAULT_PORT

logger = logging.getLogger(__name__)


class DaemonManager:
    """
    Gestor del ciclo de vida del daemon de tts-sidecar.

    Maneja start/stop/restart/status. Funciona en Windows, Linux y macOS.
    """

    START_TIMEOUT = 120.0  # La carga del modelo + compilación tarda 30-90s

    def __init__(self):
        self.system = platform.system()
        self.port = DEFAULT_PORT
        self.base_url = f"http://127.0.0.1:{self.port}"

    def start(
        self,
        background: bool = True,
        auto_restart: bool = False,
        max_retries: int = 0,
        language: str = "all",
        with_stt: bool = False,
    ) -> bool:
        """
        Inicia el daemon. Idempotente: si ya está corriendo, devuelve True.

        `language` ("es-latam", "en" o "all", default "all") se reenvía tal
        cual al subproceso del daemon (ver `run.serve`): qué modelo(s) se
        precargan en caliente al arrancar. `with_stt` (opt-in) se reenvía
        igual: el flag llega al hijo (daemon serve congelado o -m) para que
        precargue también el modelo de transcripción.

        La ventana de carrera del doble arranque se cierra
        con un lock de arranque atómico: `_acquire_start_lock()` crea el pidfile
        con `os.open(O_CREAT|O_EXCL)` antes de lanzar el subproceso, de modo que
        dos `start()` concurrentes no pueden lanzar dos daemons —el segundo ve el
        lock vigente y no arranca—. El mismo archivo persiste el PID del daemon
        (registro autoritativo que desambigua un proceso huérfano o zombie sin
        depender del escaneo por cmdline). Los locks obsoletos —PID muerto o
        ajeno, un arranque abortado, o un daemon colgado cuyo arranque superó el
        timeout sin abrir el puerto— se reclaman al validarlos con psutil.
        """
        # Si ya está corriendo no hay nada que hacer
        if self.is_running():
            print("Daemon ya está corriendo", file=sys.stderr)
            return True

        # En modo congelado el ejecutable no acepta `-m módulo`,
        # así que se auto-invoca mediante su subcomando `daemon serve`.
        if paths.is_frozen():
            cmd = [sys.executable, "daemon", "serve"]
        else:
            cmd = [
                sys.executable,
                "-m", "tts_sidecar.daemon.run",
            ]

        if auto_restart:
            cmd.append("--auto-restart")
        if max_retries > 0:
            cmd.extend(["--max-retries", str(max_retries)])
        cmd.extend(["--language", language])
        if with_stt:
            cmd.append("--with-stt")

        if background:
            # Lock de arranque atómico: serializa los `start` concurrentes
            # antes de lanzar el subproceso. Si el lock está vigente,
            # ya hay un daemon corriendo o arrancando y no se lanza otro.
            if not self._acquire_start_lock():
                print("Daemon ya está arrancando", file=sys.stderr)
                return True

            env = os.environ.copy()
            # Modo fuente/pip-installed: fijar PYTHONPATH para que el subproceso
            # encuentre tts_sidecar. En modo congelado el ejecutable ya es
            # autocontenido. La ruta calculada es el padre del paquete (`src/`
            # en modo fuente, `site-packages` en pip/uv): en ambos casos es
            # inocua y suficiente para que el import se resuelva.
            if not paths.is_frozen():
                # __file__ es .../tts_sidecar/daemon/daemon.py → 3 dirname = padre del paquete
                src_path = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
                if os.path.exists(src_path):
                    env["PYTHONPATH"] = src_path

            try:
                if self.system == "Windows":
                    # CREATE_NO_WINDOW: el hijo (python.exe o el EXE congelado
                    # --console, ambos de subsistema consola) NO crea consola
                    # alguna -> sin ventana ni parpadeo. CREATE_NEW_PROCESS_GROUP
                    # lo aísla en su propio grupo de proceso, así no hereda la
                    # consola del padre ni recibe CTRL_CLOSE al cerrar la
                    # terminal que lo lanzó (sobrevive como daemon). La
                    # combinación es válida: CREATE_NO_WINDOW solo se ignora con
                    # DETACHED_PROCESS o CREATE_NEW_CONSOLE, no con este flag.
                    proc = subprocess.Popen(
                        cmd,
                        env=env,
                        creationflags=(
                            subprocess.CREATE_NO_WINDOW
                            | subprocess.CREATE_NEW_PROCESS_GROUP
                        ),
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                    )
                else:
                    proc = subprocess.Popen(
                        cmd,
                        env=env,
                        stdout=subprocess.DEVNULL,
                        stderr=subprocess.DEVNULL,
                        start_new_session=True,
                    )
            except Exception:
                # El subproceso ni siquiera arrancó: soltar el lock.
                self._clear_pidfile()
                raise

            # Persistir el PID del daemon recién lanzado en el pidfile/lock.
            self._write_pid(proc.pid)

            # Esperar a que el daemon esté listo (la carga del modelo tarda).
            # Si expira, se conserva el pidfile: el proceso puede seguir vivo
            # cargando el modelo, y un lock realmente obsoleto (PID muerto) se
            # reclama en el próximo `start`.
            return self._wait_for_ready()
        else:
            # Modo primer plano (para depuración)
            subprocess.run(cmd)
            return True

    def stop(self, timeout: float = 10.0) -> bool:
        """
        Detiene el daemon. Devuelve True cuando el daemon ya no está corriendo.

        El mecanismo principal es el cierre graceful vía HTTP (/shutdown señaliza
        should_exit sobre uvicorn.Server). El kill por PID resuelto con psutil es
        la red de seguridad para el caso en que el proceso siga ocupando el puerto.
        """
        # Verificar si está corriendo
        if not self.is_running():
            # Aunque el health check falle, comprobar si algo ocupa el puerto
            pid = self._get_pid_from_port()
            if pid:
                self._kill_pid(pid)
                print("Daemon no está corriendo", file=sys.stderr)
                return True
            # durante la ventana de arranque (carga del modelo, 30-90 s)
            # el puerto aún está cerrado y ni el health check ni el escaneo de
            # puerto ven al daemon; reportar «no está corriendo» sería un éxito
            # falso.
            #
            # El pidfile es la fuente autoritativa. Si registra un PID vivo del
            # daemon dentro de la ventana de arranque, es un arranque en curso:
            # se avisa y se devuelve False (exit 5) sin matar el proceso. Si esa
            # ventana ya expiró, el proceso está colgado (nunca abrió el puerto):
            # se termina y se limpia el pidfile, en vez de un exit 5 perpetuo. Si
            # el PID está muerto/ajeno, es un pidfile obsoleto (zombie): se limpia
            # igual. Solo si no hay pidfile se cae al escaneo por cmdline
            # (comportamiento previo).
            pid = self._read_pid()
            if pid is not None:
                if self._pid_alive_daemon(pid):
                    if self._within_start_window(self._pidfile()):
                        self._print_starting_notice(pid)
                        return False
                    self._kill_pid(pid)
                self._clear_pidfile()
                print("Daemon no está corriendo", file=sys.stderr)
                return True

            starting = self._find_starting_daemon()
            if starting is not None:
                self._print_starting_notice(starting.pid)
                return False
            print("Daemon no está corriendo", file=sys.stderr)
            return True

        # Cierre graceful vía HTTP
        try:
            response = requests.post(
                f"{self.base_url}/shutdown",
                timeout=timeout
            )
            if not response.ok:
                print(
                    f"Advertencia: /shutdown devolvió {response.status_code}; "
                    "se recurre al kill por PID si el proceso sigue activo.",
                    file=sys.stderr,
                )
        except requests.RequestException:
            pass

        # Dar tiempo para que el cierre graceful termine
        time.sleep(0.5)

        # Si sigue corriendo, forzar terminación por PID
        if self.is_running():
            pid = self._get_pid_from_port()
            if pid:
                self._kill_pid(pid)

        return not self.is_running()

    def restart(self) -> bool:
        """Reinicia el daemon: detiene el existente y arranca uno nuevo."""
        print("Deteniendo daemon...", file=sys.stderr)
        self.stop()
        time.sleep(1)
        print("Iniciando daemon...", file=sys.stderr)
        return self.start()

    def status(self) -> dict:
        """Devuelve el estado del daemon."""
        if not self.is_running():
            return {"running": False}

        try:
            response = requests.get(f"{self.base_url}/health", timeout=5)
            if response.status_code == 200:
                data = response.json()
                return {
                    "running": True,
                    "status": data.get("status"),
                    "model_loaded": data.get("model_loaded"),
                    "uptime_seconds": data.get("uptime_seconds"),
                }
        except requests.RequestException:
            pass

        return {"running": True, "status": "unknown"}

    def is_running(self) -> bool:
        """Comprueba si el daemon está corriendo y responde al health check.

        Delegación al cliente IPC: una sola implementación del health check.
        """
        from .ipc import DaemonIPCClient

        return DaemonIPCClient().is_running()

    def _wait_for_ready(self, timeout: float = None) -> bool:
        """Espera hasta que el daemon esté listo para aceptar conexiones."""
        timeout = timeout or self.START_TIMEOUT
        start = time.time()

        print(f"Esperando que el daemon esté listo (timeout={timeout}s)...", file=sys.stderr)
        while time.time() - start < timeout:
            if self.is_running():
                print("Daemon listo", file=sys.stderr)
                return True
            time.sleep(1)

        print("Timeout esperando al daemon", file=sys.stderr)
        return False

    def _get_pid_from_port(self) -> Optional[int]:
        """Devuelve el PID del proceso que escucha en el puerto del daemon.

        Usa psutil.net_connections para una resolución uniforme en Windows,
        Linux y macOS, sin depender de herramientas específicas de cada SO
        (netstat/ss) ni de sus formatos de salida.
        """
        try:
            import psutil

            for conn in psutil.net_connections(kind="inet"):
                if (
                    conn.laddr
                    and conn.laddr.port == self.port
                    and conn.status == psutil.CONN_LISTEN
                    and conn.pid
                ):
                    return conn.pid
        except Exception:
            logger.debug("No se pudo detectar el daemon por puerto (psutil.net_connections)", exc_info=True)
        return None

    @staticmethod
    def _find_starting_daemon():
        """Busca un proceso del daemon en arranque (puerto aún cerrado) por cmdline.

        Escaneo sin estado (sin archivo PID): usa solo los markers específicos
        del daemon ('tts_sidecar.daemon', 'daemon serve') — no el genérico
        'tts-sidecar' de _is_own_daemon_process, que matchearía al propio
        comando 'stop' — y excluye el PID propio. Devuelve el proceso o None.
        """
        try:
            import psutil

            own_pid = os.getpid()
            markers = ("tts_sidecar.daemon", "daemon serve")
            for proc in psutil.process_iter():
                if proc.pid == own_pid:
                    continue
                try:
                    cmdline = " ".join(proc.cmdline())
                except psutil.Error:
                    # Proceso desaparecido/zombie/sin permiso: esperado en un
                    # escaneo; un error inesperado burbujea al handler externo.
                    continue
                if any(marker in cmdline for marker in markers):
                    return proc
        except Exception:
            logger.debug("Escaneo de daemon en arranque (psutil.process_iter) falló", exc_info=True)
        return None

    @staticmethod
    def _is_own_daemon_process(proc) -> bool:
        """Comprueba que el proceso identificado por puerto sea nuestro daemon.

        Sin esta verificación, otro servicio que ocupara el puerto 8765 sería
        terminado por 'daemon stop' (acción destructiva sobre un proceso ajeno).
        """
        try:
            cmdline = " ".join(proc.cmdline())
        except Exception:
            # Fail-closed: si no se puede leer el cmdline, no se asume que sea
            # nuestro daemon (evita terminar un proceso ajeno).
            logger.debug("No se pudo leer el cmdline del proceso; se trata como ajeno", exc_info=True)
            return False
        markers = ("tts_sidecar.daemon", "tts-sidecar", "daemon serve")
        return any(marker in cmdline for marker in markers)

    def _kill_pid(self, pid: int):
        """Termina un proceso por su PID (red de seguridad tras el cierre graceful).

        Solo actúa si el cmdline corresponde al daemon propio; intenta
        terminate() (SIGTERM/equivalente) y, si no cede, kill().
        """
        try:
            import psutil

            proc = psutil.Process(pid)
            if not self._is_own_daemon_process(proc):
                print(
                    f"El proceso {pid} en el puerto {self.port} no parece ser el daemon "
                    "de tts-sidecar; no se termina.",
                    file=sys.stderr,
                )
                return
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except psutil.TimeoutExpired:
                proc.kill()
        except Exception:
            logger.debug("No se pudo terminar el proceso del daemon por PID", exc_info=True)

    # -- PID/lock file del daemon (serializa el arranque y persiste el PID) --

    @staticmethod
    def _pidfile() -> str:
        """Ruta del PID/lock file del daemon."""
        return paths.daemon_pidfile()

    def _read_pid(self) -> Optional[int]:
        """Lee el PID del pidfile. None si no existe, está vacío o es ilegible."""
        try:
            with open(self._pidfile(), "r", encoding="utf-8") as fh:
                content = fh.read().strip()
            return int(content) if content else None
        except (OSError, ValueError):
            return None

    @staticmethod
    def _pid_alive_daemon(pid: int) -> bool:
        """True si `pid` está vivo y su cmdline es el de nuestro daemon.

        Un PID muerto (psutil.Process lanza NoSuchProcess), reutilizado por otro
        proceso, o un zombie sin cmdline no cuentan como daemon vivo.
        """
        try:
            import psutil

            return DaemonManager._is_own_daemon_process(psutil.Process(pid))
        except Exception:
            logger.debug("No se pudo verificar si el PID corresponde a un daemon vivo", exc_info=True)
            return False

    def _acquire_start_lock(self) -> bool:
        """Crea atómicamente el pidfile como lock de arranque.

        Devuelve True si adquirimos el lock (se puede lanzar el daemon), o False
        si ya hay uno corriendo/arrancando (lock vigente). Un lock obsoleto
        —PID muerto o ajeno, o un archivo vacío más viejo que el timeout de
        arranque— se reclama y se reintenta el `open` una vez.
        """
        path = self._pidfile()
        for _ in range(2):
            try:
                fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o644)
                os.close(fd)
                return True
            except FileExistsError:
                if self._reclaim_if_stale(path):
                    continue
                return False
            except OSError:
                return False
        return False

    def _within_start_window(self, path: str) -> bool:
        """True si el intento de arranque registrado en `path` sigue dentro de
        la ventana START_TIMEOUT: un arranque en curso legítimo, no un cuelgue.

        Usa el mtime del pidfile como marca del inicio del intento (lo actualiza
        `_write_pid` al persistir el PID del subproceso). Si el mtime no puede
        leerse, se asume conservadoramente que el arranque sigue en curso, para
        no terminar un proceso cuya antigüedad no se pudo determinar.
        """
        try:
            age = time.time() - os.path.getmtime(path)
        except OSError:
            return True
        return age < self.START_TIMEOUT

    def _reclaim_if_stale(self, path: str) -> bool:
        """Elimina el pidfile si está obsoleto. True si lo reclamó.

        Con un PID vivo del daemon, el lock está vigente mientras el arranque
        siga dentro de la ventana START_TIMEOUT; superada, el proceso está
        colgado (nunca abrió el puerto): se termina y se reclama el lock. Con un
        PID muerto/ajeno se reclama. Un archivo vacío/ilegible se considera un
        arranque en curso, salvo que su antigüedad supere el timeout de arranque
        (arranque abortado antes de escribir el PID).
        """
        pid = self._read_pid()
        if pid is not None:
            if self._pid_alive_daemon(pid):
                if self._within_start_window(path):
                    return False
                # Daemon colgado: se termina antes de reclamar el lock para no
                # dejarlo huérfano compitiendo por el puerto en un arranque
                # futuro.
                self._kill_pid(pid)
        elif self._within_start_window(path):
            return False
        try:
            os.unlink(path)
            return True
        except OSError:
            return False

    def _write_pid(self, pid: int) -> None:
        """Escribe el PID del daemon en el pidfile/lock (best-effort)."""
        try:
            with open(self._pidfile(), "w", encoding="utf-8") as fh:
                fh.write(str(pid))
        except OSError:
            pass

    def _clear_pidfile(self) -> None:
        """Elimina el pidfile/lock (best-effort)."""
        try:
            os.unlink(self._pidfile())
        except OSError:
            pass

    def _print_starting_notice(self, pid: int) -> None:
        """Aviso de daemon en arranque (puerto aún cerrado); no se detiene."""
        print(
            f"El daemon está arrancando (PID {pid}) y aún no acepta conexiones; "
            "no se detuvo. Reintenta 'daemon stop' cuando termine la carga del "
            "modelo (30-90 s).",
            file=sys.stderr,
        )
