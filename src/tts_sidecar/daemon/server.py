"""
Servidor FastAPI del daemon de tts-sidecar.
Expone endpoints HTTP para síntesis TTS con el modelo persistente en memoria.
"""

import base64
import binascii
import gc
import logging
import queue
import threading
from typing import Optional

from fastapi import FastAPI, HTTPException, Request, Depends
from fastapi.responses import StreamingResponse

from .. import voices, __version__
from ..exceptions import (
    SynthesisCancelled, TranslationModelMissingError, TranslationFailedError,
    TranscriptionModelMissingError, TranscriptionFailedError,
)
from .protocol import (
    SynthesizeRequest,
    TranscribeRequest,
    TranscribeResponse,
    HealthResponse,
    VoicesResponse,
    PrecomputeVoiceRequest,
    PrecomputeVoiceResponse,
    ProgressEvent,
    ResultEvent,
    ErrorEvent,
)


class DaemonState:
    """Estado del daemon inyectado en los endpoints vía Depends(get_daemon_state).

    Sustituye a los globals de módulo `_engine`/`_server`/`_start_time`: vive en
    `app.state.daemon` (no como variables reasignables de módulo), así los
    endpoints lo reciben por inyección de dependencias —testeable con
    `app.dependency_overrides`, sin ensuciar estado global compartido— y el
    composition root (`run.py`) es el único que lo puebla. No se conservan
    setters módulo-level: eso solo cambiaría la forma del global sin romper el
    acoplamiento (la «trampa del parche barato» del hallazgo).

    El daemon sirve múltiples modelos (uno por idioma, rediseño cross-lingual):
    `engines` es el registro por idioma (es-latam/en), no un motor único.
    `engine` se conserva como propiedad de compatibilidad sobre `engines["es-latam"]`
    para el código y los tests de un solo modelo.
    """

    def __init__(
        self,
        engine: Optional[object] = None,
        engines: Optional[dict] = None,
        server: Optional[object] = None,
        start_time: Optional[float] = None,
        compute_backend: str = "auto",
    ):
        self.engines: dict = dict(engines) if engines is not None else {}
        if engine is not None:
            self.engines.setdefault("es-latam", engine)
        self.server = server
        self.start_time = start_time
        self.compute_backend = compute_backend
        # Loader/servicio de traducción, construidos perezosamente en el
        # primer /synthesize que traduce (ver `_get_translation_service`):
        # un único `TranslationModelLoader` por proceso, igual que `engines`
        # cachea un motor por idioma, así una segunda petición con el mismo
        # par no vuelve a cargar el modelo CT2 desde disco.
        self.translation_loader: Optional[object] = None
        self.translation_service: Optional[object] = None
        # Loader/servicio de transcripción, construidos perezosamente en el
        # primer /transcribe (ver `_get_transcription_service`): espejo del
        # par de traducción, un único `WhisperModelLoader` por proceso.
        self.transcription_loader: Optional[object] = None
        self.transcription_service: Optional[object] = None

    @property
    def engine(self) -> Optional[object]:
        """Motor «por defecto» (es-latam), por compatibilidad con código de un solo modelo."""
        return self.engines.get("es-latam")

    @engine.setter
    def engine(self, value: Optional[object]) -> None:
        if value is None:
            self.engines.pop("es-latam", None)
        else:
            self.engines["es-latam"] = value


def get_daemon_state(request: Request) -> "DaemonState":
    """Provee a los endpoints el DaemonState alojado en app.state (DI de FastAPI)."""
    return request.app.state.daemon


def _get_translation_service(state: "DaemonState"):
    """Devuelve el `TranslationService` cacheado en `state`, construyéndolo la
    primera vez que se necesita (imports diferidos: pysbd/ctranslate2/
    sentencepiece no se cargan en un daemon que nunca traduce)."""
    if state.translation_service is None:
        from ..translation import (
            TranslationModelLoader, TranslationService, SentenceSegmenter,
            MarianTranslator, SegmentAssembler,
        )
        state.translation_loader = TranslationModelLoader()
        state.translation_service = TranslationService(
            state.translation_loader, SentenceSegmenter(),
            MarianTranslator(state.translation_loader), SegmentAssembler(),
        )
    return state.translation_service


def _get_transcription_service(state: "DaemonState"):
    """Devuelve el `TranscriptionService` cacheado en `state`, construyéndolo
    la primera vez que se necesita (imports diferidos: faster-whisper no se
    carga en un daemon que nunca transcribe). El endpoint delega en
    `WhisperTranscriber`; el motor de transcripción es agnóstico."""
    if state.transcription_service is None:
        from ..transcription import (
            WhisperModelLoader, WhisperTranscriber, TranscriptionService,
        )
        state.transcription_loader = WhisperModelLoader()
        state.transcription_service = TranscriptionService(
            state.transcription_loader, WhisperTranscriber(state.transcription_loader),
        )
    return state.transcription_service


def _clear_model_memory():
    """Libera la caché CUDA fragmentada y fuerza GC tras cada síntesis.

    Esta rutina es segura y multiplataforma:
    - `torch.cuda.empty_cache()` es un no-op si no hay dispositivo CUDA disponible
    - `gc.collect()` funciona universalmente (CPU/CUDA/MPS y todos los SO)
    - El import de `torch` es diferido y está protegido por ImportError para
      que la rutina sea inocua incluso si `torch` no está disponible

    La limpieza se ejecuta en el hilo worker tras cada síntesis (éxito o error),
    previniendo la fragmentación de memoria del daemon bajo uso prolongado en GPU.
    """
    try:
        import torch
        torch.cuda.empty_cache()
    except ImportError:
        pass  # torch no disponible: nada que limpiar
    finally:
        gc.collect()


# Aplicación FastAPI
app = FastAPI(
    title="tts-sidecar-daemon",
    description="Daemon TTS persistente con modelo cacheado en memoria",
)
# Estado inicial vacío alojado en el propio objeto app (no en globals de módulo):
# run.py lo puebla al arrancar y los tests lo sustituyen (directamente o vía
# app.dependency_overrides[get_daemon_state]). Encapsulado y sustituible.
app.state.daemon = DaemonState()


@app.get("/health", response_model=HealthResponse)
async def health_check(state: DaemonState = Depends(get_daemon_state)):
    """Endpoint de health check."""
    import time
    model_loaded = {lang: lang in state.engines for lang in ("es-latam", "en")}
    if state.translation_loader is not None:
        from ..translation import default_cache_dir
        # El par opus-mt es<->en se provisiona/reporta como un único recurso
        # (Tarea 8): "caliente" si cualquiera de las dos direcciones ya se
        # cargó en memoria (la primera traducción real de esa dirección).
        model_loaded["translate:es-en"] = state.translation_loader.is_loaded(
            default_cache_dir("es", "en")
        ) or state.translation_loader.is_loaded(default_cache_dir("en", "es"))
    if state.transcription_loader is not None:
        from ..transcription import default_cache_dir
        model_loaded["transcribe:small"] = state.transcription_loader.is_loaded(
            default_cache_dir()
        )
    return HealthResponse(
        status="healthy" if state.engines else "initializing",
        model_loaded=model_loaded,
        uptime_seconds=time.time() - state.start_time if state.start_time else 0,
        version=__version__,
    )


# Serializa la síntesis completa (preparación de conds + generate): engine.synthesize
# muta estado global del modelo (tts.conds) y dos peticiones concurrentes
# cruzarían voces.
_synthesis_lock = threading.Lock()

# Control de admisión: sin tope, una ráfaga de invocaciones concurrentes
# lanza un thread worker por petición que se apila esperando _synthesis_lock,
# saturando el proceso bajo el GIL. El semáforo acota la admisión a 1 síntesis
# activa + hasta 3 en espera; la N+1 se rechaza con 503 antes de crear thread.
MAX_INFLIGHT_SYNTHESIS = 4
_admission_semaphore = threading.BoundedSemaphore(MAX_INFLIGHT_SYNTHESIS)


@app.post("/synthesize")
def synthesize(
    req: SynthesizeRequest,
    state: DaemonState = Depends(get_daemon_state),
) -> StreamingResponse:
    """
    Sintetiza texto a audio usando el modelo cacheado en memoria.

    Endpoint síncrono (def): FastAPI lo despacha a su threadpool, de modo que
    una síntesis larga no bloquea el event loop y /health sigue respondiendo.

    Devuelve un flujo NDJSON (application/x-ndjson): N líneas `progress` con el
    avance de la síntesis (etapa y conteo de tokens del T3 en vivo) seguidas de
    una línea `result` con el WAV en base64 y los tiempos por sub-etapa; si la
    síntesis falla en el hilo worker, se emite una línea `error`. El esquema de
    cada línea lo define protocol.py (ProgressEvent/ResultEvent/ErrorEvent).

    El daemon resuelve el nombre de voz de la petición contra su propio
    registro antes de sintetizar; una voz no registrada produce un frame
    `error` (vía el `except FileNotFoundError` de más abajo), no una
    respuesta 400/503.

    `req.language` selecciona el motor: si no está caliente en `state.engines`,
    se carga perezosamente desde disco (nunca dispara una descarga), reutilizando
    la caché de `ChatterboxEngine` con el `compute_backend` fijado al arrancar
    el daemon.
    """
    engine = state.engines.get(req.language)
    if engine is None:
        from ..model_cache import model_for
        from ..engine import ChatterboxEngine

        try:
            engine = ChatterboxEngine.get_instance(
                model=model_for(req.language), compute_backend=state.compute_backend,
            )
        except Exception:
            engine = None
        else:
            state.engines[req.language] = engine

    if not engine:
        raise HTTPException(status_code=503, detail="Modelo no cargado")

    # Admisión no bloqueante: si ya hay MAX_INFLIGHT_SYNTHESIS peticiones en
    # vuelo, se rechaza de inmediato en vez de apilar otro thread worker.
    if not _admission_semaphore.acquire(blocking=False):
        raise HTTPException(
            status_code=503,
            detail="Daemon ocupado (demasiadas síntesis concurrentes), reintente en unos segundos",
        )

    # Patrón productor/consumidor: la síntesis (CPU-bound y bloqueante) corre en
    # un hilo worker que empuja eventos a una cola; el generador de la respuesta
    # los drena como líneas NDJSON hasta un centinela. Así el progreso viaja al
    # cliente mientras el T3/S3Gen siguen trabajando, sin bloquear el event loop.
    def event_stream():
        q: queue.Queue = queue.Queue()
        SENTINEL = object()
        # Evento de cancelación cooperativa ligado al estado de la conexión del
        # cliente: el generador lo setea al detectar la desconexión y el
        # push del worker lo consulta para abortar engine.synthesize().
        cancel_event = threading.Event()

        def worker():
            try:
                # La síntesis sigue serializada (una a la vez): engine.synthesize muta
                # estado global del modelo (tts.conds) y dos síntesis concurrentes
                # cruzarían voces. /health responde igual (endpoint aparte).
                with _synthesis_lock:
                    def push(ev: dict):
                        # El cliente se desconectó: abortamos la síntesis en el
                        # próximo punto cooperativo en vez de malgastar GPU/CPU.
                        if cancel_event.is_set():
                            raise SynthesisCancelled()
                        q.put(("progress", ev))

                    ref_path, speech_path = voices.voice_paths(req.voice)

                    # Traduce ANTES de sintetizar cuando el idioma de origen
                    # difiere del destino (normalizados, Desviación 5),
                    # passthrough intacto si coinciden.
                    from ..translation import resolve_language
                    source = resolve_language(req.source_language)
                    target = resolve_language(req.language)
                    text = req.text
                    if source != target:
                        text = _get_translation_service(state).translate(req.text, source, target)

                    result = engine.synthesize(
                        text=text,
                        timbre_reference=ref_path,
                        speech_reference=speech_path,
                        verbose=True,
                        progress_callback=push,
                        exaggeration=req.exaggeration,
                        cfg_weight=req.cfg_weight,
                        temperature=req.temperature,
                    )
                    # engine.synthesize devuelve un SynthesisResult (audio + métricas
                    # tipadas), no un dict suelto leído por convención de claves.
                    q.put((
                        "result",
                        {
                            "audio_b64": base64.b64encode(result.audio_bytes).decode("ascii"),
                            "t3_time": float(result.metrics.t3),
                            "s3gen_time": float(result.metrics.s3gen),
                        },
                    ))
            except SynthesisCancelled:
                # El cliente se fue a mitad de síntesis: no emitimos result ni
                # error (la conexión ya no existe para recibirlos). El finally
                # libera el semáforo y la memoria igual que en éxito/error.
                logging.getLogger(__name__).debug(
                    "synthesize: cancelada por desconexión del cliente"
                )
            except TranslationModelMissingError:
                logging.getLogger(__name__).warning(
                    "synthesize: modelo de traducción no provisionado"
                )
                q.put((
                    "error",
                    {"detail": "Modelo de traducción no provisionado. Ejecuta "
                               "'tts-sidecar setup --language en' primero."},
                ))
            except TranslationFailedError as e:
                logging.getLogger(__name__).error("synthesize: fallo de traducción: %s", e)
                q.put(("error", {"detail": "Error de traducción"}))
            except FileNotFoundError as e:
                # El detalle real (con rutas) queda solo en el log del servidor.
                logging.getLogger(__name__).warning("synthesize: recurso no encontrado: %s", e)
                q.put(("error", {"detail": "Recurso de voz no encontrado"}))
            except Exception as e:
                logging.getLogger(__name__).error("synthesize: error interno: %s", e)
                q.put(("error", {"detail": "Error interno de síntesis"}))
            finally:
                _clear_model_memory()
                _admission_semaphore.release()
                q.put((SENTINEL, None))

        t = threading.Thread(target=worker, daemon=True)
        t.start()

        try:
            while True:
                kind, payload = q.get()
                if kind is SENTINEL:
                    break
                if kind == "progress":
                    yield ProgressEvent(
                        stage=payload.get("stage"),
                        tokens=payload.get("tokens"),
                        elapsed=payload.get("elapsed"),
                    ).model_dump_json() + "\n"
                elif kind == "result":
                    yield ResultEvent(**payload).model_dump_json() + "\n"
                elif kind == "error":
                    yield ErrorEvent(**payload).model_dump_json() + "\n"
        except (GeneratorExit, OSError):
            # El cliente cerró la conexión (o el stream se rompió): señalizamos
            # la cancelación al worker para que deje de síntetizar y libera sus
            # recursos vía el finally. No reintentamos yield tras la desconexión.
            cancel_event.set()

    return StreamingResponse(event_stream(), media_type="application/x-ndjson")


@app.post("/transcribe", response_model=TranscribeResponse)
def transcribe(
    req: TranscribeRequest,
    state: DaemonState = Depends(get_daemon_state),
) -> TranscribeResponse:
    """Transcribe muestras PCM int16 (base64) a texto usando el modelo Whisper.

    Endpoint síncrono (def): FastAPI lo despacha a su threadpool, igual que
    /synthesize, sin worker ni colas propios. La petición nunca lleva rutas
    (invariante sin-paths): viaja el audio ya capturado por el cliente.

    Fail-fast del modelo: se valida la provisión y se carga ANTES de decodificar
    el base64 — si el modelo no está, se responde 503 sin tocar el audio.
    """
    from ..transcription import default_cache_dir
    service = _get_transcription_service(state)

    try:
        state.transcription_loader.load(default_cache_dir())
    except TranscriptionModelMissingError:
        raise HTTPException(
            status_code=503,
            detail="Modelo de transcripción no provisionado. Ejecuta "
                   "'tts-sidecar setup --with-stt' primero.",
        )
    except Exception as e:
        logging.getLogger(__name__).error("transcribe: fallo de carga del modelo: %s", e)
        raise HTTPException(status_code=500, detail="Error al cargar el modelo de transcripción")

    try:
        audio_bytes = base64.b64decode(req.audio_b64, validate=True)
    except (ValueError, binascii.Error) as e:
        raise HTTPException(status_code=400, detail=f"audio_b64 no decodificable: {e}")

    import numpy as np
    from ..audio import INT16_MAX_F
    samples = np.frombuffer(audio_bytes, dtype=np.int16).astype(np.float32) / INT16_MAX_F

    try:
        text = service.transcribe_samples(samples, req.source_language)
    except TranscriptionFailedError as e:
        logging.getLogger(__name__).error("transcribe: fallo de transcripción: %s", e)
        raise HTTPException(status_code=500, detail="Error de transcripción")

    return TranscribeResponse(text=text)


def _any_engine(state: "DaemonState") -> Optional[object]:
    """Devuelve cualquier motor caliente: list_voices/precompute_voice son
    operaciones sobre el registro de voces, no específicas de un idioma."""
    return next(iter(state.engines.values()), None)


@app.get("/voices", response_model=VoicesResponse)
async def list_voices(state: DaemonState = Depends(get_daemon_state)):
    """Lista las voces registradas."""
    engine = _any_engine(state)
    if not engine:
        raise HTTPException(status_code=503, detail="Modelo no cargado")

    return VoicesResponse(voices=engine.list_voices())


@app.post("/voices/precompute", response_model=PrecomputeVoiceResponse)
def precompute_voice(
    req: PrecomputeVoiceRequest,
    state: DaemonState = Depends(get_daemon_state),
) -> PrecomputeVoiceResponse:
    """Precomputa y guarda los conditionals de una voz ya registrada.

    Endpoint síncrono (def): FastAPI lo despacha a su threadpool. El precómputo
    corre bajo `_synthesis_lock` porque comparte el modelo con la síntesis
    (forward passes sobre tts.ve/s3gen/t3); serializarlo evita contención en el
    dispositivo con una síntesis en vuelo. El engine lee los audios desde el
    registro (voice_paths), dentro de los directorios permitidos.
    """
    engine = _any_engine(state)
    if not engine:
        raise HTTPException(status_code=503, detail="Modelo no cargado")

    try:
        with _synthesis_lock:
            engine.precompute_voice(req.name)
    except FileNotFoundError as e:
        # El detalle real (con rutas) queda solo en el log del servidor.
        logging.getLogger(__name__).warning("precompute_voice: voz no encontrada: %s", e)
        raise HTTPException(status_code=404, detail="Voz no encontrada")
    except Exception as e:
        logging.getLogger(__name__).error("precompute_voice: error interno: %s", e)
        raise HTTPException(status_code=500, detail="Error interno de precómputo")

    return PrecomputeVoiceResponse(name=req.name, precomputed=True)


@app.post("/shutdown")
async def shutdown(state: DaemonState = Depends(get_daemon_state)):
    """Endpoint de cierre graceful del daemon.

    Señaliza `should_exit` sobre la instancia de uvicorn.Server para que el
    servidor termine su ciclo de vida de forma ordenada. Se responde antes de
    que uvicorn cierre: el flag se procesa en la siguiente iteración del loop.

    Libera la referencia al engine (en el DaemonState inyectado) y fuerza la
    misma limpieza de memoria (`_clear_model_memory`) que corre tras cada
    síntesis: sin esto, un auto-restart frecuente del daemon
    podía dejar memoria GPU retenida entre reinicios porque nada liberaba el
    engine en el apagado. Simétrico por diseño: mismo helper, mismas garantías
    (no-op sin CUDA, gc.collect() incondicional).

    Riesgo aceptado: no lleva token ni confirmación explícita.
    El daemon bindea exclusivamente a 127.0.0.1 (ver run.py), por lo que solo
    un proceso con acceso local a la máquina puede invocarlo; se acepta ese
    riesgo residual en vez de añadir un secreto que el propio cliente IPC
    tendría que gestionar y persistir.
    """
    if state.server is not None:
        state.server.should_exit = True
        # Libera las referencias a los motores (permite al GC recolectar los
        # tensores/modelos que retienen) y limpia la caché CUDA fragmentada,
        # igual que al final de cada síntesis.
        state.engines.clear()
        _clear_model_memory()
        return {"status": "shutting_down"}
    # Sin instancia registrada (no debería ocurrir): el kill por PID es la red de seguridad.
    raise HTTPException(status_code=503, detail="Servidor no disponible para apagado graceful")
