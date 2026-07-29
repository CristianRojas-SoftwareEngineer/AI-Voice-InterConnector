"""
Rutas de datos del proyecto.

Fuente única de verdad para las raíces de datos, con un modelo uniforme válido
en los tres modos de ejecución (fuente, instalado vía pip/uv, congelado
PyInstaller): se distinguen dos raíces según su propósito.

- **Raíz de usuario** (`data_root`): estable y **escribible** entre
  ejecuciones, donde viven las voces de usuario. Es siempre el directorio de
  datos de usuario por SO, independientemente del modo de ejecución.
- **Raíz de fábrica** (`bundled_voices_dir`): de **solo lectura**, con las
  voces empaquetadas (p. ej. la voz de fábrica `default`). Es siempre el
  subdirectorio `voices/` relativo a este paquete: en modo fuente y
  pip/uv-installed es `tts_sidecar/voices/` dentro del árbol del paquete; en
  modo congelado (PyInstaller onedir) es el mismo subdirectorio dentro de
  `sys._MEIPASS`, donde el bundle extrae los datos incluidos vía
  `--add-data ...:tts_sidecar/voices` (ver `scripts/build_utils.py`).

El build nativo es **onedir** (no onefile): el ejecutable convive con su
directorio `_internal/`. Se usa el user-data-dir por SO para las voces de
usuario porque el directorio de instalación puede ser de solo lectura
(p. ej. `Program Files`, `/Applications`, `site-packages`), no por un temporal
efímero de extracción.

Ninguna función aquí importa ni carga el modelo: son operaciones puras de
sistema de archivos.
"""

import os
import sys
from pathlib import Path


def is_frozen() -> bool:
    """True cuando corremos dentro del ejecutable compilado (onedir)."""
    return getattr(sys, "frozen", False)


def data_root() -> str:
    """
    Raíz de datos estable y escribible: el directorio de datos de usuario por
    SO (creado si no existe), igual en los tres modos de ejecución.
    """
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~\\AppData\\Local")
    elif sys.platform == "darwin":
        base = os.path.expanduser("~/Library/Application Support")
    else:
        base = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")

    root = os.path.join(base, "tts-sidecar")
    Path(root).mkdir(parents=True, exist_ok=True)
    return root


def daemon_pidfile() -> str:
    """
    Ruta del PID/lock file del daemon, en la raíz de datos de usuario.

    Cumple dos funciones a la vez: (1) lock de arranque —se crea de forma
    atómica con `os.open(O_CREAT|O_EXCL)` para serializar `daemon start` y
    cerrar la ventana de carrera del doble arranque— y (2) registro del PID del
    daemon, que desambigua un proceso huérfano o zombie sin depender del escaneo
    por cmdline. Vive en `data_root()` porque debe ser escribible y estable
    entre ejecuciones en los tres modos.
    """
    return os.path.join(data_root(), "daemon.pid")


def bundled_voices_dir() -> str:
    """
    Directorio de voces de fábrica empaquetadas (solo lectura).

    Congelado: `sys._MEIPASS/tts_sidecar/voices`, donde PyInstaller extrae los
    datos empaquetados vía `--add-data`.
    Fuente y pip/uv-installed: el subdirectorio `voices` de este mismo paquete.
    """
    if is_frozen():
        meipass = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
        return os.path.join(meipass, "tts_sidecar", "voices")

    return os.path.join(os.path.dirname(os.path.abspath(__file__)), "voices")
