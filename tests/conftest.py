"""Fixtures de pytest para los tests de ai-voice-interconnector."""

import os
import sys
from pathlib import Path

# Asegura que src/ esté en el path para imports relativos al proyecto
sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

# numpy 2.x llama a os.uname() cuando sys.platform es "linux". Los tests que
# parchean sys.platform para simular Linux (p. ej. TestSetupUninstall) pueden
# importar numpy por primera vez dentro de esa ventana parcheada, y en un host
# Windows real os.uname() no existe (AttributeError). Fijar la variable hace
# que numpy salte esa rama, con independencia del orden de selección de tests.
os.environ.setdefault("NUMPY_MADVISE_HUGEPAGE", "0")


def pytest_configure(config):
    """Corre el bootstrap antes de la recolección.

    Asegura que la supresión de warnings (incl. `pkg_resources`) esté activa
    antes de que los tests importen módulos como `ai_voice_interconnector.audio` a nivel
    de módulo, de modo que la supresión no dependa del filtro local de
    audio.py (eliminado). `bootstrap.apply()` es idempotente.
    """
    import ai_voice_interconnector.bootstrap
    ai_voice_interconnector.bootstrap.apply()
