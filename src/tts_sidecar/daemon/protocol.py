"""
Definiciones del protocolo IPC del daemon de tts-sidecar.

`SynthesizeRequest` (el cuerpo del POST /synthesize) NO cambia respecto a la
variante binaria previa. Lo que cambia es la RESPUESTA de `/synthesize`: ahora es
un flujo NDJSON (una línea JSON por evento) en vez de un único cuerpo binario WAV.
El orden garantizado del stream es N×`progress` → 1×`result` (con el WAV en base64),
o bien 1×`error` si la síntesis falla en el hilo worker. El esquema de cada línea
lo definen `ProgressEvent` / `ResultEvent` / `ErrorEvent` (abajo), fuente única de
verdad validada por ambos extremos: `server.py` (productor) emite vía
`model_dump_json()` e `ipc.py` (consumidor) valida cada línea con `model_validate` y
aborta con `DaemonIPCError` ante cualquier frame no conforme (sin tolerancia).
Ver `server.py::synthesize` para el productor y `docs/DAEMON-MODE.md` para el
protocolo completo.
"""

from pydantic import BaseModel, ConfigDict, Field
from typing import Literal, Optional

# Tope de texto por petición: acota el trabajo del T3 y evita el DoS local
# trivial de un payload ilimitado.
MAX_TEXT_LENGTH = 5000

# Tope de longitud del nombre de voz: holgado sobre el límite de nombre de
# archivo de los tres SO (255), suficiente para acotar el payload antes de que
# la resolución en el registro (voices.voice_paths) lo valide de verdad.
MAX_VOICE_NAME_LENGTH = 255


class ProtocolModel(BaseModel):
    """Clase base de los modelos del protocolo daemon↔cliente (NDJSON + REST).

    Centraliza la política de compatibilidad hacia adelante y hacia atrás en
    un solo punto, en vez de dejarla como el default implícito de Pydantic en
    cada modelo:
      - `schema_version`: subió a "2" con el cambio incompatible de
        `SynthesizeRequest`, que dejó de aceptar rutas de audio crudas y pasó
        a recibir el nombre de una voz registrada. Mientras los cambios sean
        aditivos (campo nuevo con default) no hace falta incrementarlo: un
        cliente/daemon viejo que no lo conozca simplemente lo ignora al
        parsear; un cambio incompatible de un campo existente sí lo amerita.
      - `extra="ignore"`: un campo desconocido en el payload (p. ej. un daemon
        más nuevo enviando un campo que este proceso aún no conoce) se
        descarta en vez de fallar la validación. Sin esto, el skew de
        versiones entre un daemon residente y un CLI recién actualizado (o
        viceversa) rompería la comunicación en vez de degradar con gracia.
    `SynthesizeRequest` (el único modelo de petición cliente→daemon, no de
    stream) NO hereda de esta base: su validación es deliberadamente estricta
    (ver su propio docstring).
    """
    model_config = ConfigDict(extra="ignore")

    schema_version: str = "2"


class SynthesizeRequest(BaseModel):
    """Request de síntesis de habla.

    El daemon sirve un único modelo y compute backend fijados al arrancar; la
    petición no lleva `model` ni `compute_backend` (el servidor los ignoraría).
    La petición lleva el nombre de una voz ya registrada; el daemon resuelve
    sus rutas de audio desde su propio registro (nunca una ruta del llamador).
    """
    text: str = Field(min_length=1, max_length=MAX_TEXT_LENGTH)
    voice: str = Field(min_length=1, max_length=MAX_VOICE_NAME_LENGTH)


# ---------------------------------------------------------------------------
# Esquema del stream NDJSON de /synthesize (contrato daemon→cliente).
#
# Cada línea del stream es exactamente uno de estos tres modelos, discriminados
# por el campo `event`. El cliente parsea cada línea, lee `event` y actúa:
#   - "progress": avance de la síntesis (etapa actual y, durante el T3, tokens).
#   - "result":  frame final con el WAV en base64 y los tiempos por sub-etapa.
#   - "error":   la síntesis falló en el hilo worker; el cliente lo convierte en
#                DaemonIPCError (exit 5 en el CLI). No expone rutas del sistema.
# ---------------------------------------------------------------------------


class ProgressEvent(ProtocolModel):
    """Evento de avance de la síntesis (una línea `progress` del stream)."""
    event: Literal["progress"] = "progress"
    stage: Optional[str] = None
    """Etapa actual: conditionals, tts, t3, s3gen, encoding, saving."""
    tokens: Optional[int] = None
    """Conteo de tokens del T3 en vivo (solo durante la etapa 't3')."""
    elapsed: Optional[float] = None
    """Segundos transcurridos desde el inicio de la síntesis (opcional)."""


class ResultEvent(ProtocolModel):
    """Frame final del stream: el WAV completo y los tiempos por sub-etapa."""
    event: Literal["result"] = "result"
    audio_b64: str
    """WAV PCM 24kHz mono codificado en base64 (ASCII)."""
    t3_time: float = 0.0
    s3gen_time: float = 0.0


class ErrorEvent(ProtocolModel):
    """Frame de error del stream: la síntesis falló en el hilo worker."""
    event: Literal["error"] = "error"
    detail: str
    """Mensaje seguro para el cliente (sin rutas del sistema)."""


class HealthResponse(ProtocolModel):
    """Respuesta del health check."""
    status: str
    """Estado del daemon: 'healthy', 'initializing' o 'error'."""
    model_loaded: bool
    """True cuando el modelo está completamente cargado en memoria."""
    uptime_seconds: float
    """Segundos transcurridos desde el inicio del daemon."""
    version: str = ""
    """Versión del paquete tts-sidecar que sirve este daemon (__version__).
    Cadena vacía por defecto: un daemon que aún no la puebla (skew hacia
    atrás) no rompe la validación de is_running()."""


class VoicesResponse(ProtocolModel):
    """Lista de voces registradas."""
    voices: list[str]


class PrecomputeVoiceRequest(ProtocolModel):
    """Petición de precómputo de conditionals para una voz ya registrada.

    Solo lleva el nombre: el daemon resuelve los audios desde el registro
    (voices.voice_paths), dentro de sus directorios permitidos, así que la
    petición nunca transporta rutas del sistema de archivos del cliente.
    """
    name: str = Field(min_length=1, max_length=MAX_VOICE_NAME_LENGTH)


class PrecomputeVoiceResponse(ProtocolModel):
    """Resultado del precómputo: la voz y si se escribieron sus conditionals."""
    name: str
    precomputed: bool
