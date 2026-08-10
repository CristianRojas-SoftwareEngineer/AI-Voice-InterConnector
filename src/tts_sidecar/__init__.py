"""
TTS Sidecar — síntesis de voz con clonación de voz.
100% local, licencia GPL-3.0-or-later, soporte para español latinoamericano.

Copyright (C) 2026 Cristián Rojas Arredondo

Este programa es software libre: puedes redistribuirlo y/o modificarlo bajo los
términos de la GNU General Public License publicada por la Free Software
Foundation, ya sea la versión 3 de la licencia o (a tu elección) cualquier
versión posterior. Se distribuye SIN NINGUNA GARANTÍA. Consulta el archivo
LICENSE para el texto completo, o <https://www.gnu.org/licenses/>.
"""

__version__ = "0.10.2"
__author__ = "Cristián Rojas Arredondo"
__license__ = "GPL-3.0-or-later"

# Imports perezosos: permite ejecutar --help sin que las dependencias pesadas estén instaladas
def __getattr__(name):
    """
    Resuelve imports perezosos de los símbolos públicos del paquete.

    ChatterboxEngine y AudioPlayer se importan solo cuando se acceden por primera
    vez, evitando cargar torch/chatterbox al invocar subcomandos ligeros como
    --help, version o devices.
    """
    if name == "ChatterboxEngine":
        from .engine import ChatterboxEngine
        return ChatterboxEngine
    if name == "AudioPlayer":
        from .audio import AudioPlayer
        return AudioPlayer
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")

# API pública real: ambos símbolos son imports perezosos resueltos por
# __getattr__ arriba; declararlos explícitamente evita que `__all__ = []`
# contradiga lo que el paquete expone de facto (p. ej. `from tts_sidecar import *`).
__all__ = ["ChatterboxEngine", "AudioPlayer"]
