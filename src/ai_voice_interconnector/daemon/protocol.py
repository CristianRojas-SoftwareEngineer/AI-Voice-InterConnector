"""
Definiciones del protocolo IPC del daemon de ai-voice-interconnector.

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

from pydantic import BaseModel, ConfigDict, Field, model_validator
from typing import Literal, Optional

# Tope de texto por petición: acota el trabajo del T3 y evita el DoS local
# trivial de un payload ilimitado.
MAX_TEXT_LENGTH = 5000

# Tope de audio por petición de transcripción: derive de `REQUEST_TIMEOUT`
# (ipc.py), el único timeout que gobierna /transcribe: 300.0 s de PCM int16
# crudo a 16 kHz/mono (300 × 32 000 B/s = 9 600 000 B) expandido por el factor
# base64 4/3. Autoconsistente con la decisión cerrada de wire format int16.
MAX_AUDIO_BYTES = 12_800_000

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

    schema_version: str = "3"


class SynthesizeRequest(BaseModel):
    """Request de síntesis de habla.

    El daemon sirve múltiples modelos (uno por idioma, rediseño cross-lingual);
    la petición lleva `language` para elegir cuál usar, además de los tres
    overrides opcionales de síntesis (`None` = default de la ruta del idioma,
    ver `ChatterboxEngine.SYNTHESIS_DEFAULTS`). No lleva `compute_backend`
    (fijado al arrancar el daemon). La petición lleva el nombre de una voz ya
    registrada; el daemon resuelve sus rutas de audio desde su propio registro
    (nunca una ruta del llamador).

    `source_language` (aditivo, sin subir `schema_version`) es el idioma del
    texto de entrada; `None` se resuelve a `language` (mismo idioma, sin
    traducir) vía el `model_validator` de abajo, así la invariante "sin origen
    explícito, el origen es el destino" se cumple sin importar si la petición
    se construyó en el flujo directo o se deserializó del JSON entrante del
    daemon.
    """
    text: str = Field(min_length=1, max_length=MAX_TEXT_LENGTH)
    voice: str = Field(min_length=1, max_length=MAX_VOICE_NAME_LENGTH)
    language: str = "es-latam"
    source_language: Optional[str] = None
    exaggeration: Optional[float] = None
    cfg_weight: Optional[float] = None
    temperature: Optional[float] = None

    @model_validator(mode="after")
    def _resolve_source_language(self) -> "SynthesizeRequest":
        if self.source_language is None:
            self.source_language = self.language
        return self


class TranscribeRequest(BaseModel):
    """Request de transcripción de habla.

    El daemon no recibe rutas (invariante sin-paths): la petición transporta
    las muestras ya decodificadas como PCM int16 crudo a 16 kHz/mono (wire
    format de la decisión cerrada de Fase 4/5) en base64 ASCII, más el idioma
    hablado en el audio. `source_language` es la taxonomía CLI (`es-latam`/`en`),
    normalizada internamente por `resolve_language` en el servidor.

    Modelo aditivo: NO sube `schema_version` (un daemon viejo ignora el campo
    desconocido vía `extra="ignore"` del lado del endpoint inexistente; la
    ausencia de la ruta la detecta el cliente como daemon de versión antigua).
    """
    audio_b64: str = Field(min_length=1, max_length=MAX_AUDIO_BYTES)
    source_language: str = "es-latam"


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
    model_loaded: dict[str, bool] = Field(default_factory=dict)
    """Qué modelos están calientes en RAM, por idioma (es-latam/en). Un idioma
    ausente del dict no está precargado (se cargaría perezosamente al usarlo)."""
    uptime_seconds: float
    """Segundos transcurridos desde el inicio del daemon."""
    version: str = ""
    """Versión del paquete ai-voice-interconnector que sirve este daemon (__version__).
    Cadena vacía por defecto: un daemon que aún no la puebla (skew hacia
    atrás) no rompe la validación de is_running()."""


class VoicesResponse(ProtocolModel):
    """Lista de voces registradas."""
    voices: list[str]


class TranscribeResponse(ProtocolModel):
    """Respuesta de transcripción: el texto transcrito del audio recibido."""
    text: str


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
