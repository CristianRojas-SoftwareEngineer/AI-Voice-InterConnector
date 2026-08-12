"""Permite `python -m ai_voice_interconnector` como vía de invocación equivalente al entry point.

Invoca la capa única de bootstrap explícitamente antes de importar el CLI,
simétrico con `bin/ai-voice-interconnector` y `python -m ai_voice_interconnector.daemon.run`.
"""

from . import bootstrap
bootstrap.apply()

from .cli import main

if __name__ == "__main__":
    main()
