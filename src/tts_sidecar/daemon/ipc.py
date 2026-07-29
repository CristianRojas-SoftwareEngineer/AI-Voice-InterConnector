"""
Cliente IPC para comunicarse con el daemon de tts-sidecar.
Usa HTTP sobre TCP (funciona en todas las plataformas).
"""

import base64
import binascii
import json
from typing import Callable, Optional

import requests
from pydantic import ValidationError

from ..timing import SynthesisMetrics, SynthesisResult, log

# Puerto fijo del daemon (loopback 127.0.0.1). Fuente única importada por
# daemon.py y run.py: no existe flag --port. Correr dos daemons o convivir con
# otro servicio en este puerto no está soportado por diseño (ver DAEMON-MODE.md).
DEFAULT_PORT = 8765


class DaemonIPCError(Exception):
    """Error de comunicación IPC con el daemon."""
    pass


class DaemonIPCClient:
    """
    Cliente IPC para comunicarse con el daemon de tts-sidecar.

    Usa HTTP sobre TCP (127.0.0.1:8765), lo que garantiza compatibilidad
    con Windows, Linux y macOS sin depender de Unix sockets ni named pipes.
    """

    TIMEOUT = 5.0          # Timeout de conexión
    # Timeout de síntesis: 5 min por defecto (audio largo en CPU lenta).
    # Sobreescribible con TTS_SIDECAR_REQUEST_TIMEOUT (segundos, float) para
    # consumidores programáticos que prefieran fallar antes; un valor
    # inválido o no positivo se ignora y se conserva el default.
    REQUEST_TIMEOUT = 300.0

    def __init__(self):
        self.port = DEFAULT_PORT
        self.base_url = f"http://127.0.0.1:{self.port}"
        import os
        raw = os.environ.get("TTS_SIDECAR_REQUEST_TIMEOUT")
        if raw:
            try:
                value = float(raw)
                if value > 0:
                    self.REQUEST_TIMEOUT = value
            except ValueError:
                pass

    def is_running(self) -> bool:
        """Comprueba si el daemon está corriendo y responde al health check.

        No basta con un 200: si otro servicio local ocupara el puerto 8765 y
        respondiera 200, un chequeo por status code lo confundiría con nuestro
        daemon (falso «ya corriendo» y síntesis posteriores fallando con exit 5
        difícil de atribuir). Por eso el cuerpo se valida estrictamente contra el
        modelo canónico `HealthResponse` de `protocol.py`; cualquier cuerpo que no
        conforme ese esquema se trata como «no es nuestro daemon» → `False`. A
        diferencia del resto de consumidores IPC, esta sonda no eleva
        `DaemonIPCError`: discriminar deliberadamente un servicio ajeno del puerto
        es su contrato, no un fallo silenciado.
        """
        from ..daemon.protocol import HealthResponse

        try:
            response = requests.get(
                f"{self.base_url}/health",
                timeout=self.TIMEOUT
            )
            if response.status_code != 200:
                return False
            try:
                HealthResponse.model_validate(response.json())
            except (ValidationError, ValueError):
                # Cuerpo ausente, no-JSON o que no valida el esquema: no es nuestro daemon.
                return False
            return True
        except (requests.ConnectionError, requests.Timeout):
            return False

    def synthesize(
        self,
        text: str,
        voice: str,
        on_progress: Optional[Callable[[dict], None]] = None,
    ) -> SynthesisResult:
        """
        Sintetiza texto vía daemon, consumiendo el stream NDJSON.

        El daemon responde con N líneas `progress` (avance de la síntesis)
        seguidas de una línea `result` con el WAV en base64; una línea `error`
        indica un fallo del hilo de síntesis. Cada línea `progress` se reenvía a
        `on_progress` (si se da) para alimentar el indicador de progreso del CLI.

        Args:
            on_progress: Callback opcional invocado con el dict de cada evento
                `progress` del stream (mismo contrato que engine.speak).

        Devuelve un `SynthesisResult` (audio decodificado del frame `result` +
        métricas t3/s3gen), la misma forma que devuelve `engine.speak()` en modo
        directo.

        Raises:
            DaemonIPCError: Si la comunicación falla o el daemon emite `error`.
        """
        try:
            response = requests.post(
                f"{self.base_url}/synthesize",
                json={
                    "text": text,
                    "voice": voice,
                },
                timeout=self.REQUEST_TIMEOUT,
                stream=True,
            )

            # Los errores de validación (400/503) llegan como respuesta inmediata
            # con cuerpo JSON {"detail": ...}, no como frame del stream.
            if response.status_code != 200:
                try:
                    error = response.json().get("detail", "Error desconocido")
                except ValueError:
                    # Cuerpo de error no-JSON: no romper la promesa de DaemonIPCError
                    error = f"HTTP {response.status_code}"
                raise DaemonIPCError(f"Error del daemon: {error}")

            audio_bytes: Optional[bytes] = None
            metrics = SynthesisMetrics()
            for raw in response.iter_lines():
                if not raw:
                    # Línea vacía (keep-alive HTTP): no es un frame, se salta.
                    continue
                try:
                    payload = json.loads(raw)
                except ValueError as e:
                    raise DaemonIPCError(
                        f"El daemon emitió una línea no-JSON: {e}"
                    )

                # Validación estricta contra la única fuente de verdad del
                # contrato (protocol.py). Un `event` desconocido o un frame con
                # esquema inválido aborta la síntesis en vez de tolerarse.
                from ..daemon.protocol import (
                    ProgressEvent,
                    ResultEvent,
                    ErrorEvent,
                )

                kind = payload.get("event") if isinstance(payload, dict) else None
                try:
                    if kind == "progress":
                        ev = ProgressEvent.model_validate(payload)
                    elif kind == "result":
                        ev = ResultEvent.model_validate(payload)
                    elif kind == "error":
                        ev = ErrorEvent.model_validate(payload)
                    else:
                        raise DaemonIPCError(
                            f"Evento de stream desconocido del daemon: {kind!r}"
                        )
                except ValidationError as e:
                    raise DaemonIPCError(
                        f"El daemon devolvió un frame no conforme: {e}"
                    )

                if kind == "progress":
                    if on_progress is not None:
                        on_progress(ev.model_dump())
                elif kind == "result":
                    try:
                        audio_bytes = base64.b64decode(
                            ev.audio_b64, validate=True
                        )
                    except (ValueError, binascii.Error) as e:
                        raise DaemonIPCError(
                            f"El frame 'result' tiene audio_b64 no decodificable: {e}"
                        )
                    # Timing por sub-etapa (equivalente a los antiguos headers
                    # X-T3-Time / X-S3Gen-Time). Se emite con el MISMO formato que
                    # el modo directo (engine.py usa estas cadenas exactas) y vía
                    # log(), coordinado con el spinner activo, sobre stderr.
                    t3_time = ev.t3_time
                    s3gen_time = ev.s3gen_time
                    if t3_time is not None and s3gen_time is not None:
                        log(f"   [Etapa 2a] T3 autoregresivo: {float(t3_time):.1f}s")
                        log(f"   [Etapa 2b] S3Gen vocoder:   {float(s3gen_time):.1f}s")
                        metrics = SynthesisMetrics(t3=float(t3_time), s3gen=float(s3gen_time))
                elif kind == "error":
                    raise DaemonIPCError(
                        f"Error del daemon: {ev.detail}"
                    )

            if audio_bytes is None:
                # El stream terminó sin frame `result` ni `error`: contrato roto.
                raise DaemonIPCError("El daemon no devolvió audio")
            return SynthesisResult(audio_bytes=audio_bytes, metrics=metrics)

        except requests.ConnectionError as e:
            raise DaemonIPCError(f"No se puede conectar al daemon: {e}")
        except requests.Timeout as e:
            raise DaemonIPCError(f"Timeout del daemon: {e}")
        except requests.RequestException as e:
            raise DaemonIPCError(f"Error de comunicación con el daemon: {e}")

    def list_voices(self) -> list[str]:
        """Lista las voces registradas vía daemon.

        Valida el cuerpo contra `VoicesResponse` (protocol.py); un cuerpo no
        conforme eleva `DaemonIPCError` en vez de degradarse silenciosamente a `[]`.
        """
        from ..daemon.protocol import VoicesResponse

        try:
            response = requests.get(
                f"{self.base_url}/voices",
                timeout=self.TIMEOUT
            )
            if response.status_code != 200:
                return []
            try:
                body = VoicesResponse.model_validate(response.json())
            except (ValidationError, ValueError) as e:
                raise DaemonIPCError(
                    f"El daemon devolvió un cuerpo /voices no conforme: {e}"
                )
            return body.voices
        except (requests.ConnectionError, requests.Timeout):
            return []

    def precompute_voice(self, name: str) -> bool:
        """Precomputa los conditionals de una voz registrada vía daemon.

        Aprovecha el modelo caliente del daemon (evita una carga en frío por
        clon). Usa REQUEST_TIMEOUT porque el precómputo puede tardar en CPU.
        Valida el cuerpo contra `PrecomputeVoiceResponse` (protocol.py); un
        error HTTP o un cuerpo no conforme elevan `DaemonIPCError`.
        """
        from ..daemon.protocol import PrecomputeVoiceResponse

        try:
            response = requests.post(
                f"{self.base_url}/voices/precompute",
                json={"name": name},
                timeout=self.REQUEST_TIMEOUT,
            )
            if response.status_code != 200:
                try:
                    error = response.json().get("detail", "Error desconocido")
                except ValueError:
                    error = f"HTTP {response.status_code}"
                raise DaemonIPCError(f"Error del daemon: {error}")
            try:
                body = PrecomputeVoiceResponse.model_validate(response.json())
            except (ValidationError, ValueError) as e:
                raise DaemonIPCError(
                    f"El daemon devolvió un cuerpo /voices/precompute no conforme: {e}"
                )
            return body.precomputed
        except requests.ConnectionError as e:
            raise DaemonIPCError(f"No se puede conectar al daemon: {e}")
        except requests.Timeout as e:
            raise DaemonIPCError(f"Timeout del daemon: {e}")
        except requests.RequestException as e:
            raise DaemonIPCError(f"Error de comunicación con el daemon: {e}")


def is_daemon_running() -> bool:
    """
    Comprueba si el daemon está corriendo y responde al health check.

    Función de conveniencia que crea un DaemonIPCClient temporal para
    verificar el estado sin necesidad de mantener una instancia del cliente.
    """
    try:
        client = DaemonIPCClient()
        return client.is_running()
    except Exception:
        return False
