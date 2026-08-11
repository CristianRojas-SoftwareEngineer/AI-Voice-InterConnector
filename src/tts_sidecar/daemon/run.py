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


def serve(
    port: int = DEFAULT_PORT,
    auto_restart: bool = False,
    max_retries: int = 0,
    language: str = "all",
    with_stt: bool = False,
):
    """
    Arranca el servidor del daemon en primer plano (bloqueante).

    Reutilizable tanto por `main()` (modo `python -m tts_sidecar.daemon.run`)
    como por el subcomando `daemon serve` del ejecutable congelado.

    `language` ("es-latam", "en" o "all", default "all") fija qué modelo(s) se
    precargan en caliente al arrancar (rediseño cross-lingual, §3.9); el resto
    se carga perezosamente desde disco al primer uso vía `/synthesize`.

    `with_stt` (opt-in) precarga también el modelo de transcripción
    faster-whisper-small; el gate de provisión en disco corre en el proceso
    padre (`cmd_daemon`), aquí solo se calienta lo ya provisionado.
    """
    # Idiomas a precargar en caliente.
    languages_to_preload = ["es-latam", "en"] if language == "all" else [language]

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
            # Etapa 1: cargar modelo(s)
            with StageTimer("1-Daemon", "Etapa 1/3: Cargando modelo"):
                from ..engine import ChatterboxEngine
                from ..compute_backend import ComputeBackendResolver
                from ..model_cache import model_for

                # El daemon decide el compute backend una sola vez al
                # arrancar y lo cachea en la instancia del motor: cualquier
                # síntesis posterior reutiliza esa decisión. TTS_SIDECAR_COMPUTE_BACKEND
                # es el override de bajo nivel; con "auto" (o sin la var),
                # ComputeBackendResolver.resolve() detecta cuda → mps → cpu.
                compute_backend = ComputeBackendResolver.resolve(
                    os.environ.get("TTS_SIDECAR_COMPUTE_BACKEND")
                )
                app.state.daemon.compute_backend = compute_backend

                # El engine ya aplica los parámetros de síntesis optimizados,
                # el timing por sub-etapa (_synthesis_metrics) y el bypass del
                # watermark como comportamiento propio. Se precarga en caliente
                # cada idioma pedido (--language, default "all" = ambos).
                for lang in languages_to_preload:
                    app.state.daemon.engines[lang] = ChatterboxEngine.get_instance(
                        model=model_for(lang),
                        compute_backend=compute_backend,
                    )

                log(f"Daemon: compute_backend={compute_backend}")

                # Warmup: la precarga de arriba solo hace load_state_dict().to(device).eval(),
                # nunca ejecuta un forward. La init perezosa del runtime (contexto CUDA +
                # autotune cuDNN en GPU; pool oneDNN/MKL en CPU) se dispara recién en el
                # primer forward real. Se ejecuta aquí una síntesis mínima descartable por
                # idioma precargado para pagar esa init en el arranque en vez de en la
                # primera petición del usuario. Best-effort: la voz de fábrica se resuelve
                # una sola vez y, si falla o falla alguna síntesis, se loguea y se continúa
                # sin abortar el arranque del daemon.
                from .. import voices

                try:
                    warmup_timbre, warmup_speech = voices.voice_paths("default")
                except Exception as e:
                    log(f"Daemon: warmup omitido, voz de fábrica no resuelta: {e}")
                else:
                    for lang in languages_to_preload:
                        try:
                            app.state.daemon.engines[lang].synthesize(
                                "hola",
                                timbre_reference=warmup_timbre,
                                speech_reference=warmup_speech,
                                verbose=False,
                            )
                        except Exception as e:
                            log(f"Daemon: warmup falló para '{lang}': {e}")
                            continue

                # Precarga en caliente del par de traducción opus-mt es<->en
                # (Tarea 11): con --language en/all, ambas direcciones quedan
                # calientes desde el arranque en vez de esperar a la primera
                # petición /synthesize que traduzca. `_require_models_cached_for_daemon`
                # ya validó en el proceso padre (cmd_daemon) que el par está
                # provisionado en disco antes de llegar aquí.
                if language in ("en", "all"):
                    from ..translation import (
                        TranslationModelLoader,
                        TranslationService,
                        SentenceSegmenter,
                        MarianTranslator,
                        SegmentAssembler,
                        default_cache_dir,
                    )

                    translation_loader = TranslationModelLoader()
                    app.state.daemon.translation_loader = translation_loader
                    app.state.daemon.translation_service = TranslationService(
                        translation_loader, SentenceSegmenter(),
                        MarianTranslator(translation_loader), SegmentAssembler(),
                    )
                    for source, target in (("es", "en"), ("en", "es")):
                        translation_loader.load(default_cache_dir(source, target))

                # Precarga en caliente del modelo de transcripción
                # faster-whisper-small (Fase 5): con --with-stt el modelo
                # queda caliente desde el arranque en vez de pagar la carga
                # fría en la primera petición /transcribe. El proceso padre
                # (cmd_daemon) ya validó la provisión en disco; aquí no hay
                # lógica de descarga, solo se calienta lo provisionado.
                if with_stt:
                    from .server import _get_transcription_service
                    from ..transcription import default_cache_dir

                    _get_transcription_service(app.state.daemon)
                    app.state.daemon.transcription_loader.load(default_cache_dir())

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

        # Invalida las instancias cacheadas para forzar una recarga real de
        # cada motor precargado: si el crash se debió a un estado interno
        # corrupto, revivir el mismo objeto anularía el propósito de --auto-restart.
        from ..engine import ChatterboxEngine
        from ..compute_backend import ComputeBackendResolver
        from ..model_cache import model_for
        for lang in languages_to_preload:
            ChatterboxEngine._cache.pop(
                ComputeBackendResolver.cache_key(model=model_for(lang), compute_backend=compute_backend),
                None,
            )

        time.sleep(1)

    log("Daemon: detenido")


def main():
    """
    Punto de entrada CLI del daemon.

    Parsea argumentos y delega en serve(). Se invoca como:
        python -m tts_sidecar.daemon.run [--auto-restart] [--max-retries N] [--language {es-latam,en,all}]

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
    parser.add_argument(
        "--language",
        choices=["es-latam", "en", "all"],
        default="all",
        help="Idioma(s) a precargar en caliente (default: all = ambos)"
    )
    parser.add_argument(
        "--with-stt",
        action="store_true",
        help="Precarga el modelo de transcripción faster-whisper-small "
             "(requiere setup --with-stt; opt-in)"
    )
    args = parser.parse_args()

    serve(
        auto_restart=args.auto_restart,
        max_retries=args.max_retries,
        language=args.language,
        with_stt=args.with_stt,
    )


if __name__ == "__main__":
    main()
