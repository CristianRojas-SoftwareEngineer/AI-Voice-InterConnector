"""Excepciones compartidas del motor y del daemon de ai-voice-interconnector.

Módulo deliberadamente libre de imports pesados (no importa torch ni
chatterbox) para que el servidor del daemon pueda importar el tipo de
cancelación sin arrastrar el motor completo.
"""


class SynthesisCancelled(Exception):
    """Señal cooperativa de cancelación de una síntesis en curso.

    La eleva el callback de progreso del daemon (``push`` del worker de
    ``/synthesize``) al detectar que el cliente se desconectó, y el engine la
    deja propagar desde ``_emit_progress`` / ``_token_counting_iter`` en vez de
    tragarla como las demás excepciones del callback (best-effort). Así el
    worker puede abortar ``engine.synthesize()`` en el próximo punto cooperativo.
    """


class TranslationModelMissingError(Exception):
    """El modelo de traducción convertido a CT2 no está en la ruta de caché
    esperada. La eleva `TranslationModelLoader.load()` cuando la ruta no
    existe; el llamador la mapea a `EXIT_MODEL_MISSING` con un mensaje que
    remite a `setup`."""


class UnsupportedLanguagePairError(Exception):
    """El par (origen, destino) solicitado no está entre los pares soportados
    por el subsistema de traducción (`es`/`en`). Identidad propia distinta de
    `TranslationModelMissingError`: aquí el modelo simplemente no existe para
    ese par, no es un problema de provisión."""


class TranslationFailedError(Exception):
    """El backend de traducción (runtime CT2 en producción, doble en tests)
    falló al traducir un segmento. Identidad propia para no colapsarla con
    `TranslationModelMissingError`: aquí el modelo sí cargó, pero la
    inferencia falló. La eleva `MarianTranslator.translate()`."""


class TranscriptionModelMissingError(Exception):
    """El modelo de transcripción (faster-whisper) no está en la ruta de
    caché esperada. La eleva `WhisperModelLoader.load()` cuando la ruta no
    existe; el llamador la mapea a `EXIT_MODEL_MISSING` con un mensaje que
    remite a `setup --with-stt`."""


class TranscriptionFailedError(Exception):
    """El backend de transcripción (faster-whisper en producción, doble en
    tests) falló al transcribir el audio con el modelo ya cargado. Identidad
    propia para no colapsarla con `TranscriptionModelMissingError`: aquí el
    modelo sí cargó, pero la inferencia falló. La eleva
    `WhisperTranscriber.transcribe()`."""
