"""Gobernanza del contrato de salida (§2.9).

Dos invariantes tri-partitas que sostienen el estado canónico del contrato:

1. Ningún `EXIT_*` se define fuera de `exit_codes.py`: el módulo hoja es la
   única fuente de verdad, de modo que nadie redefina una constante ni abra un
   ciclo de import.
2. La tabla pública de `USAGE.md` y el módulo dicen lo mismo: cada código del
   contrato aparece como fila de la tabla, y ninguna fila inventa un código que
   el módulo no declare.
"""

import re
from pathlib import Path

from tts_sidecar import exit_codes

_SRC = Path(__file__).parent.parent / "src" / "tts_sidecar"
_USAGE = Path(__file__).parent.parent / "USAGE.md"

# Asignación de una constante EXIT_* a nivel de módulo: `EXIT_FOO = 3`.
_EXIT_ASSIGN_RE = re.compile(r"^\s*EXIT_[A-Z_]+\s*=", re.MULTILINE)


def _module_codes():
    """Conjunto de valores enteros de las constantes EXIT_* del módulo hoja."""
    return {
        value
        for name, value in vars(exit_codes).items()
        if name.startswith("EXIT_") and isinstance(value, int)
    }


def test_no_exit_constant_defined_outside_exit_codes():
    """Ningún módulo del paquete redefine un EXIT_* fuera de exit_codes.py."""
    offenders = []
    for path in _SRC.rglob("*.py"):
        if path.name == "exit_codes.py":
            continue
        if _EXIT_ASSIGN_RE.search(path.read_text(encoding="utf-8")):
            offenders.append(str(path.relative_to(_SRC)))
    assert offenders == [], (
        "EXIT_* debe declararse solo en exit_codes.py; definido también en: "
        + ", ".join(offenders)
    )


def test_usage_table_matches_module_codes():
    """La primera columna de la tabla de códigos de USAGE.md == valores del módulo."""
    text = _USAGE.read_text(encoding="utf-8")
    # Filas de la tabla de contrato: `| \`N\` | ... | ... |` con N entero.
    table_codes = {
        int(m) for m in re.findall(r"^\s*\|\s*`(\d+)`\s*\|", text, re.MULTILINE)
    }
    assert table_codes == _module_codes(), (
        "La tabla de USAGE.md y exit_codes.py deben declarar los mismos códigos; "
        f"solo en el módulo: {_module_codes() - table_codes}; "
        f"solo en la tabla: {table_codes - _module_codes()}"
    )
