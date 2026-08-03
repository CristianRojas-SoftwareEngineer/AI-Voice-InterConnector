"""
Entry point del daemon de tts-sidecar.

Uso:
    python -m tts_sidecar.daemon.run
"""

from .. import bootstrap
bootstrap.apply()

import argparse
import atexit
import errno
import logging
import os
import signal
import sys
import time

import uvicorn

from .server import app, DaemonState
from .ipc import DEFAULT_PORT
from ..exit_codes import EXIT_ERROR, EXIT_STATE_CONFLICT
from ..timing import StageTimer, log

logger = logging.getLogger(__name__)


def _remove_own_pidfile():
    """Elimina el PID/lock file del daemon si registra nuestro propio PID.

    El pidfile lo crea `DaemonManager.start()` con el PID de este proceso; al
    cerrar (graceful o por señal) lo borramos para soltar el lock. La guarda por
    PID evita que un proceso ajeno borre un pidfile que no es suyo.
    """
    from .. import paths

    try:
        path = paths.daemon_pidfile()
        with open(path, "r", encoding="utf-8") as fh:
            content = fh.read().strip()
        # Cerrar antes de borrar: Windows no permite eliminar un archivo abierto.
        if content == str(os.getpid()):
            os.remove(path)
    except OSError:
        pass


def serve(port: int = DEFAULT_PORT, auto_restart: bool = False, max_retries: int = 0):
    """
    Arranca el servidor del daemon en primer plano (bloqueante).

    Reutilizable tanto por `main()` (modo `python -m tts_sidecar.daemon.run`)
    como por el subcomando `daemon serve` del ejecutable congelado.
    """
    # Registrar intentos de reinicio
    retries = 0

    def signal_handler(signum, frame):
        log("Daemon: señal de cierre recibida")
        sys.exit(0)

    # Estos handlers solo cubren la ventana entre este registro y el arranque
    # de server.run(): uvicorn.Server instala sus propios manejadores de
    # SIGTERM/SIGINT al iniciar su event loop y, desde ese momento, es su
    # mecanismo (should_exit) el que gobierna el apagado ordenado.
    signal.signal(signal.SIGTERM, signal_handler)
    signal.signal(signal.SIGINT, signal_handler)

    # Soltar el lock al salir: atexit corre en el cierre normal y en sys.exit()
    # (el SystemExit que dispara signal_handler ante SIGTERM/SIGINT). Un cierre
    # abrupto (SIGKILL, os._exit) deja el pidfile, pero el próximo `start` lo
    # reclama al validar que el PID ya no está vivo.
    atexit.register(_remove_own_pidfile)

    while True:
        # Composition root: se construye un DaemonState fresco por iteración
        # (relevante en --auto-restart) y se puebla conforme se crean el engine
        # y el uvicorn.Server. Los endpoints lo reciben vía Depends, no como
        # global de módulo.
        app.state.daemon = DaemonState(start_time=time.time())

        with StageTimer("Startup", "Iniciando daemon..."):
            # Etapa 1: cargar modelo
            with StageTimer("1-Daemon", "Etapa 1/3: Cargando modelo"):
                from ..engine import ChatterboxEngine
                from ..compute_backend import ComputeBackendResolver

                # El daemon decide el compute backend una sola vez al
                # arrancar y lo cachea en la instancia del motor: cualquier
                # síntesis posterior reutiliza esa decisión. TTS_SIDECAR_COMPUTE_BACKEND
                # es el override de bajo nivel; con "auto" (o sin la var),
                # ComputeBackendResolver.resolve() detecta cuda → mps → cpu.
                compute_backend = ComputeBackendResolver.resolve(
                    os.environ.get("TTS_SIDECAR_COMPUTE_BACKEND")
                )

                # El engine ya aplica los parámetros de síntesis optimizados,
                # el timing por sub-etapa (_synthesis_metrics) y el bypass del
                # watermark como comportamiento propio.
                engine = ChatterboxEngine.get_instance(
                    model="es-mx-latam",
                    compute_backend=compute_backend,
                )

                log(f"Daemon: compute_backend={compute_backend}")
                app.state.daemon.engine = engine

            # Etapa 2: iniciar servidor
            with StageTimer("2-Daemon", "Etapa 2/3: Iniciando servidor"):
                log(f"Daemon listo en http://127.0.0.1:{port}")

            # Etapa 3: startup completo
            # El with vacío cierra el StageTimer de "Startup" total; el log de
            # duración acumulada se imprime al salir del bloque exterior.
            with StageTimer("3-Daemon", "Etapa 3/3: Startup completo"):
                pass

        if auto_restart and max_retries > 0 and retries >= max_retries:
            log(f"Daemon: máximo de intentos alcanzado ({max_retries}). Saliendo.")
            break

        try:
            # Instancia explícita de Server (en lugar de uvicorn.run) para que el
            # endpoint /shutdown pueda señalizar should_exit y cerrar de forma ordenada.
            config = uvicorn.Config(
                app,
                host="127.0.0.1",
                port=port,
                log_level="info",
                access_log=False,
            )
            server = uvicorn.Server(config)
            app.state.daemon.server = server
            server.run()
        except OSError as e:
            # Capturamos OSError antes del handler genérico para distinguir
            # «dirección ya en uso» de forma multiplataforma: errno 98 en POSIX
            # (EADDRINUSE) y 10048 en Windows (WSAEADDRINUSE).
            if e.errno in (errno.EADDRINUSE, 10048) or "address already in use" in str(e).lower():
                log(
                    f"Daemon: el puerto {port} ya está en uso. Detén el daemon "
                    f"en ejecución con 'tts-sidecar daemon stop' e intenta de nuevo."
                )
                sys.exit(EXIT_STATE_CONFLICT)
            log(f"Daemon: no se pudo enlazar el puerto {port}: {e}")
            sys.exit(EXIT_ERROR)
        except KeyboardInterrupt:
            break
        except Exception as e:
            # Mensaje legible por el usuario (stderr) + traza completa a debug
            # para diagnóstico sin ensuciar la salida normal del daemon.
            log(f"Daemon: error: {e}")
            logger.debug("Daemon: excepción no controlada en serve()", exc_info=True)

        if not auto_restart:
            break

        retries += 1
        log(f"Daemon: reiniciando (intento {retries})...")

        # Invalida la instancia cacheada para forzar una recarga real del
        # motor: si el crash se debió a un estado interno corrupto, revivir
        # el mismo objeto anularía el propósito de --auto-restart.
        from ..engine import ChatterboxEngine
        from ..compute_backend import ComputeBackendResolver
        ChatterboxEngine._cache.pop(
            ComputeBackendResolver.cache_key(model="es-mx-latam", compute_backend=compute_backend),
            None,
        )

        time.sleep(1)

    log("Daemon: detenido")


def main():
    """
    Punto de entrada CLI del daemon.

    Parsea argumentos y delega en serve(). Se invoca como:
        python -m tts_sidecar.daemon.run [--auto-restart] [--max-retries N]

    El puerto es fijo (DEFAULT_PORT = 8765 en loopback); no hay flag --port.
    """
    parser = argparse.ArgumentParser(description="tts-sidecar daemon")
    parser.add_argument(
        "--auto-restart",
        action="store_true",
        help="Reiniciar automáticamente tras un crash"
    )
    parser.add_argument(
        "--max-retries",
        type=int,
        default=0,
        help="Máximo de intentos de reinicio (0 = infinito)"
    )
    args = parser.parse_args()

    serve(
        auto_restart=args.auto_restart,
        max_retries=args.max_retries,
    )


if __name__ == "__main__":
    main()
