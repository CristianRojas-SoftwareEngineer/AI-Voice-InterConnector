"""Excepciones compartidas del motor y del daemon de tts-sidecar.

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
