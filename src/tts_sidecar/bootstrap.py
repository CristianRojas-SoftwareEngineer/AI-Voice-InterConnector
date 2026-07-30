"""
Bootstrap pre-import de tts-sidecar.

Fuente única de la preparación que debe correr antes de importar cualquier
dependencia pesada (chatterbox, perth, transformers): supresión de warnings,
variables de entorno, niveles de logging y el mock de `pkg_resources` que
Python 3.13 necesita porque el módulo fue eliminado de la stdlib pero `perth`
(dependencia de chatterbox) lo importa en tiempo de import.

`apply()` es idempotente y debe invocarse al principio de cada vía de entrada
del proceso (entry point pip/uv, `bin/tts-sidecar`, `python -m tts_sidecar`,
subcomando congelado `daemon serve`), antes de cualquier otro import del
paquete que pueda arrastrar `chatterbox`/`perth` transitivamente.
"""

import importlib.machinery
import importlib.util
import logging
import os
import sys
import types
import warnings
from pathlib import Path

_applied = False

# allow-list explícita de warnings silenciados en el arranque.
# NO usamos un catch-all `warnings.filterwarnings("ignore")` (ni
# `PYTHONWARNINGS=ignore`), porque enmascararía deprecaciones propias y de
# terceros y erosionaría la observabilidad. Cada entrada silencia un único
# warning benigno de una dependencia, y es el punto único y auditable de la
# lista de silencios. Ver la sección «Warnings silenciados» de CLAUDE.md.
#
# Formato de cada entrada: (message, category, module)
#   - ("pkg_resources is deprecated", Warning): lo emite `perth` (dep. de
#     chatterbox) al importar `pkg_resources` en Python 3.13. `category=Warning`
#     (no DeprecationWarning) porque `perth` lo emite como UserWarning en esta
#     versión; con Warning queda acotado por mensaje y cubre ambas categorías.
#   - (None, DeprecationWarning, r"^diffusers\."): el warning de
#     `LoRACompatibleLinear` al importar `chatterbox`; se filtra por módulo
#     para no atarnos al texto exacto del mensaje. El warning real es
#     FutureWarning (no DeprecationWarning), así que la categoría del
#     filtro debe coincidir.
_SILENCED_WARNINGS: list[tuple[str | None, type[Warning], str | None]] = [
    ("pkg_resources is deprecated", Warning, None),
    (None, FutureWarning, r"^diffusers\."),
    (None, FutureWarning, r"^torch\."),
]


def _install_pkg_resources_mock() -> None:
    """Instala un mock mínimo de `pkg_resources` si no está disponible.

    El mock debe ser un módulo real con `__spec__`: un objeto bare haría que
    cualquier llamada posterior a `importlib.util.find_spec('pkg_resources')`
    lanzara "pkg_resources.__spec__ is not set" (p. ej. desde el subcomando
    congelado `daemon serve`, que corre en el mismo proceso que el entry point
    del CLI y reconsulta el spec).
    """
    if 'pkg_resources' in sys.modules:
        return
    if importlib.util.find_spec('pkg_resources') is not None:
        return

    def _resource_filename(package, resource):
        spec = importlib.util.find_spec(package)
        if spec and spec.submodule_search_locations:
            return str(Path(spec.submodule_search_locations[0]) / resource)
        return resource

    mock = types.ModuleType('pkg_resources')
    mock.resource_filename = _resource_filename
    mock.__spec__ = importlib.machinery.ModuleSpec('pkg_resources', None)
    sys.modules['pkg_resources'] = mock


def apply() -> None:
    """Aplica el bootstrap pre-import. Idempotente: una segunda invocación es no-op.

    Es la **capa única** de preparación del proceso: todas las vías de entrada
    (entry point pip/uv `tts_sidecar.cli:main`, `bin/tts-sidecar`,
    `python -m tts_sidecar`, `python -m tts_sidecar.daemon.run` y el subcomando
    congelado `daemon serve`) la invocan explícitamente como su primera acción,
    en vez de depender de un efecto colateral de importación de `cli.py`.
    """
    global _applied
    if _applied:
        return
    _applied = True

    # UTF-8 primero, antes de warnings/env/imports pesados: fuerza una
    # codificación de salida consistente en toda plataforma aunque algo falle
    # temprano. Antes vivía solo en cli.py; al formar parte de la capa única,
    # el daemon y `python -m` heredan el mismo contrato de codificación.
    for stream in (sys.stdout, sys.stderr):
        reconfigure = getattr(stream, "reconfigure", None)
        if reconfigure:
            try:
                reconfigure(encoding="utf-8")
            except (ValueError, OSError):
                # Un stream ya leído, cerrado o sin reconfiguración de encoding
                # (algunos wrappers de captura/redirección) no debe abortar el
                # arranque: se conserva la codificación por defecto.
                pass

    # allow-list explícita en vez de catch-all. Silencia solo los
    # warnings benignos declarados en `_SILENCED_WARNINGS`, preservando la
    # visibilidad de cualquier otra deprecación (propia o de terceros).
    for _msg, _cat, _mod in _SILENCED_WARNINGS:
        warnings.filterwarnings(
            "ignore", message=_msg or "", category=_cat, module=_mod or ""
        )
    os.environ["HF_HUB_DISABLE_IMPLICIT_TOKEN"] = "1"
    os.environ["TRANSFORMERS_VERBOSITY"] = "error"
    os.environ["TRANSFORMERS_NO_ADVISORY_WARNINGS"] = "1"
    os.environ["TOKENIZERS_PARALLELISM"] = "false"

    logging.getLogger("huggingface_hub").setLevel(logging.ERROR)
    logging.getLogger("chatterbox.models.tokenizers.tokenizer").setLevel(logging.ERROR)
    logging.getLogger("chatterbox.models.t3.inference.alignment_stream_analyzer").setLevel(logging.ERROR)

    _install_pkg_resources_mock()
