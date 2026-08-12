#!/usr/bin/env python3
"""Gate de cobertura diferenciado por módulo.

`coverage.py` solo ofrece un `fail_under` global, que promediaría los módulos
de contrato (CLI, daemon, resolución de voces/rutas/modelo) con código
defensivo por-SO estructuralmente inalcanzable desde un solo runner —la
«métrica vanidosa» que se rechaza en este diseño—. Este script lee
`coverage.json` (generado por `pytest --cov-report=json`, ver
`[tool.coverage.*]` en pyproject.toml) y exige un piso por módulo de contrato,
tomado de `MODULE_FLOORS` (fuente única). Los módulos fuera de esa tabla se
reportan sin gatear. Homólogo a `scripts/check_third_party_licenses.py`.
"""

import json
import sys
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent

# Fuente única de los pisos de cobertura por módulo de contrato. Valores
# fijados por ratchet-desde-lo-medido: floor() del
# percent_covered observado al correr `pytest tests/ --cov-report=json`, para
# que el gate arranque en verde con la suite actual y prevenga regresiones.
MODULE_FLOORS = {
    "ai_voice_interconnector/cli.py": 83.0,
    "ai_voice_interconnector/daemon/server.py": 91.0,
    "ai_voice_interconnector/daemon/daemon.py": 63.0,
    "ai_voice_interconnector/daemon/ipc.py": 81.0,
    "ai_voice_interconnector/daemon/protocol.py": 100.0,
    "ai_voice_interconnector/daemon/run.py": 95.0,
    "ai_voice_interconnector/model_cache.py": 85.0,
    "ai_voice_interconnector/voices.py": 95.0,
    "ai_voice_interconnector/paths.py": 95.0,
}


def _normalize(path: str) -> str:
    """Normaliza separadores de ruta (coverage.json puede usar '\\' en Windows)."""
    return path.replace("\\", "/")


def load_module_coverage(json_path: Path) -> dict:
    """Parsea `coverage.json` y devuelve {ruta_normalizada: percent_covered}."""
    data = json.loads(Path(json_path).read_text(encoding="utf-8"))
    result = {}
    for raw_path, file_data in data["files"].items():
        normalized = _normalize(raw_path)
        # Recorta a partir de 'ai_voice_interconnector/' para independizarse de la ruta
        # absoluta/relativa con la que coverage.py registró el archivo.
        marker = "ai_voice_interconnector/"
        idx = normalized.rfind(marker)
        if idx != -1:
            normalized = normalized[idx:]
        result[normalized] = file_data["summary"]["percent_covered"]
    return result


def check(coverage: dict, floors: dict) -> list:
    """Devuelve la lista de (módulo, actual, piso) que incumplen su piso."""
    failures = []
    for module, floor in floors.items():
        actual = coverage.get(module)
        if actual is None:
            failures.append((module, 0.0, floor))
            continue
        if actual < floor:
            failures.append((module, actual, floor))
    return failures


def main(argv: list) -> int:
    if len(argv) != 1:
        print("uso: python scripts/check_coverage.py <coverage.json>", file=sys.stderr)
        return 1

    json_path = Path(argv[0])
    coverage = load_module_coverage(json_path)
    failures = check(coverage, MODULE_FLOORS)

    print("Cobertura por módulo de contrato (gateada):")
    for module, floor in sorted(MODULE_FLOORS.items()):
        actual = coverage.get(module, 0.0)
        marker = "OK" if actual >= floor else "FALLA"
        print(f"  [{marker}] {module}: {actual:.1f}% (piso {floor:.1f}%)")

    non_gated = sorted(set(coverage) - set(MODULE_FLOORS))
    if non_gated:
        print("\nCobertura del resto de módulos (reportada, sin gate):")
        for module in non_gated:
            print(f"  {module}: {coverage[module]:.1f}%")

    if failures:
        print("\nEl gate de cobertura falló para:", file=sys.stderr)
        for module, actual, floor in failures:
            print(f"  {module}: {actual:.1f}% < piso {floor:.1f}%", file=sys.stderr)
        print(
            "\nRegenera coverage.json con `pytest tests/ --cov --cov-report=json` "
            "y sube la cobertura, o si el descenso es deliberado, baja el piso "
            "correspondiente en scripts/check_coverage.py::MODULE_FLOORS.",
            file=sys.stderr,
        )
        return 1

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
