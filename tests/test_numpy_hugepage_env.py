"""Regresión: NUMPY_MADVISE_HUGEPAGE en conftest.py.

Sin esa variable, numpy 2.x llama a os.uname() cuando sys.platform es
"linux", aunque el host real sea Windows. Los tests que parchean
sys.platform para simular Linux (p. ej. TestSetupUninstall) podían
importar numpy por primera vez dentro de esa ventana parcheada y
estallar con AttributeError. El fallo solo se manifiesta cuando numpy no
está ya cargado en la sesión (un import previo lo enmascara), por eso se
reproduce en un subproceso aislado.
"""

import os
import subprocess
import sys

# Parchea sys.platform a linux e importa numpy: sin la variable de entorno
# heredada de conftest.py, numpy intenta os.uname() y aborta.
_SNIPPET = "import sys; sys.platform='linux'; import numpy; print('ok')"


def test_var_hugepage_fijada_en_entorno():
    assert os.environ.get("NUMPY_MADVISE_HUGEPAGE") == "0"


def test_numpy_import_under_patched_platform_no_crash():
    result = subprocess.run(
        [sys.executable, "-c", _SNIPPET],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr
    assert "ok" in result.stdout
