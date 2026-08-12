"""Tests de build_windows.build_windows() con subprocesos mockeados.

Cubren los dos comportamientos nuevos del rediseño del pipeline: la flag
`--no-installer` (que separa la etapa Installer para que el CI la corra como
step aparte) y la fatalidad del fallo del instalador (un build sin instalador
nunca debe reportar éxito).
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
import build_windows


def _patch_common(monkeypatch):
    """Neutraliza PyInstaller y las utilidades que tocan el disco real."""
    monkeypatch.setattr(build_windows, "run_pyinstaller", lambda args, timeout: 0)
    monkeypatch.setattr(build_windows, "copy_license_files", lambda dest: None)
    monkeypatch.setattr(build_windows, "bundle_size_mb", lambda onedir: 0.0)


def test_no_installer_skips_installer_stage(monkeypatch):
    """--no-installer no debe invocar el generador del instalador."""
    _patch_common(monkeypatch)
    invocaciones = []
    monkeypatch.setattr(
        build_windows.subprocess, "run",
        lambda *a, **k: invocaciones.append(a),
    )

    build_windows.build_windows("x86_64", no_installer=True)

    # La única llamada a subprocess.run del flujo es la etapa Installer, omitida.
    assert invocaciones == []


def test_installer_failure_is_fatal(monkeypatch):
    """Un instalador que falla (rc != 0) debe abortar con SystemExit(1)."""
    _patch_common(monkeypatch)

    class Result:
        returncode = 1

    monkeypatch.setattr(build_windows.subprocess, "run", lambda *a, **k: Result())

    with pytest.raises(SystemExit) as exc:
        build_windows.build_windows("x86_64", no_installer=False)
    assert exc.value.code == 1


def test_installer_success_completes(monkeypatch):
    """Con el instalador en rc 0, el build completa sin SystemExit."""
    _patch_common(monkeypatch)

    class Result:
        returncode = 0

    monkeypatch.setattr(build_windows.subprocess, "run", lambda *a, **k: Result())

    # No debe lanzar: el flujo llega al final normalmente.
    build_windows.build_windows("x86_64", no_installer=False)


def test_version_file_passed_to_pyinstaller(monkeypatch):
    """--version-file debe pasarse a PyInstaller con una ruta que existe en disco
    (metadata PE de identidad, endurecimiento contra la heurística de antivirus)."""
    monkeypatch.setattr(build_windows, "copy_license_files", lambda dest: None)
    monkeypatch.setattr(build_windows, "bundle_size_mb", lambda onedir: 0.0)

    capturados = []

    def _fake_run_pyinstaller(args, timeout):
        capturados.append(args)
        return 0

    monkeypatch.setattr(build_windows, "run_pyinstaller", _fake_run_pyinstaller)

    build_windows.build_windows("x86_64", no_installer=True)

    assert len(capturados) == 1
    args = capturados[0]
    assert "--version-file" in args
    version_file = Path(args[args.index("--version-file") + 1])
    assert version_file.exists()
    contenido = version_file.read_text(encoding="utf-8")
    assert "AI Voice InterConnector" in contenido
    assert "GPL-3.0-or-later" in contenido
