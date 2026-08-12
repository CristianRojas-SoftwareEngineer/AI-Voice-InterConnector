"""Tests estáticos de los templates generados por build_linux.py .

Calcan el patrón de tests/test_create_installer_windows.py: aserciones de
cadena sobre las funciones puras que emiten los artifacts (AppRun, .desktop).
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
from build_linux import _apprun_script, _desktop_entry


def test_apprun_shebang_and_delegation():
    """El script AppRun debe tener shebang POSIX y delegar en el ejecutable
    del bundle pasando todos los argumentos ("$@")."""
    script = _apprun_script()
    assert script.startswith("#!/bin/sh\n")
    assert 'HERE="$(dirname "$(readlink -f "$0")")"' in script
    assert 'exec "$HERE/usr/bin/ai-voice-interconnector" "$@"' in script


def test_desktop_entry_application_type_and_terminal():
    """El .desktop debe ser Type=Application y Terminal=true (salida CLI visible)."""
    desktop = _desktop_entry()
    assert "Type=Application" in desktop
    assert "Name=ai-voice-interconnector" in desktop
    assert "Exec=ai-voice-interconnector" in desktop
    assert "Icon=ai-voice-interconnector" in desktop
    assert "Terminal=true" in desktop
    assert desktop.count("\n") >= 6  # al menos 6 líneas (clave=valor)


class TestEnsureRuntimeDependencies:
    """Las ramas de fallo de ensure_runtime_dependencies abortan el build
    con SystemExit(1) en vez de propagar un traceback crudo."""

    def test_missing_lockfile_aborts(self, tmp_path, monkeypatch):
        import build_linux

        monkeypatch.setattr(build_linux, "PROJECT_ROOT", tmp_path)
        with pytest.raises(SystemExit) as exc:
            build_linux.ensure_runtime_dependencies("x86_64")
        assert exc.value.code == 1

    def test_pip_failure_aborts(self, tmp_path, monkeypatch):
        import subprocess
        import build_linux

        monkeypatch.setattr(build_linux, "PROJECT_ROOT", tmp_path)
        (tmp_path / "requirements-lock-linux-cpu.txt").write_text("", encoding="utf-8")

        def _falla(cmd, **kwargs):
            raise subprocess.CalledProcessError(returncode=2, cmd=cmd)

        monkeypatch.setattr(build_linux.subprocess, "run", _falla)
        with pytest.raises(SystemExit) as exc:
            build_linux.ensure_runtime_dependencies("x86_64")
        assert exc.value.code == 1

    def test_pip_timeout_aborts(self, tmp_path, monkeypatch):
        import subprocess
        import build_linux

        monkeypatch.setattr(build_linux, "PROJECT_ROOT", tmp_path)
        # arm64 usa el lock universal, no el CPU-only: se cubre de paso la selección.
        (tmp_path / "requirements-lock.txt").write_text("", encoding="utf-8")

        def _expira(cmd, **kwargs):
            raise subprocess.TimeoutExpired(cmd=cmd, timeout=1)

        monkeypatch.setattr(build_linux.subprocess, "run", _expira)
        with pytest.raises(SystemExit) as exc:
            build_linux.ensure_runtime_dependencies("arm64")
        assert exc.value.code == 1


def test_appimage_failure_is_fatal(tmp_path, monkeypatch):
    """appimagetool con rc != 0 debe abortar el build con SystemExit(1),
    heredando la consola (sin capture_output) para que su output sea visible."""
    import build_linux

    dist = tmp_path / "dist"
    build = tmp_path / "build"
    dist.mkdir()
    build.mkdir()
    onedir = dist / "ai-voice-interconnector"
    onedir.mkdir()
    (onedir / "ai-voice-interconnector").write_text("bin", encoding="utf-8")

    monkeypatch.setattr(build_linux, "DIST_DIR", dist)
    monkeypatch.setattr(build_linux, "BUILD_DIR", build)
    monkeypatch.setattr(build_linux, "run_pyinstaller", lambda args, timeout: 0)
    monkeypatch.setattr(build_linux, "bundle_size_mb", lambda o: 0.0)
    monkeypatch.setattr(build_linux, "copy_license_files", lambda d: None)
    monkeypatch.setattr(build_linux, "ensure_png_icon", lambda p: p)
    monkeypatch.setattr(build_linux, "get_version", lambda: "9.9.9")

    fake_tool = tmp_path / "appimagetool"
    fake_tool.write_text("x", encoding="utf-8")
    fake_runtime = tmp_path / "runtime"
    fake_runtime.write_text("x", encoding="utf-8")
    monkeypatch.setattr(
        build_linux, "provision_appimage_tooling",
        lambda arch: (fake_tool, fake_runtime),
    )

    captured = {}

    class Result:
        returncode = 1

    def fake_run(*a, **k):
        captured.update(k)
        return Result()

    monkeypatch.setattr(build_linux.subprocess, "run", fake_run)

    with pytest.raises(SystemExit) as exc:
        build_linux.build_linux("x86_64")
    assert exc.value.code == 1
    assert "capture_output" not in captured


def test_appimage_tooling_missing_degrades_without_abort(tmp_path, monkeypatch):
    """provision_appimage_tooling que devuelve None (tooling no descargable)
    debe degradar el stage AppImage con warning y NO abortar el build: el
    onedir de PyInstaller sigue siendo usable. Espejo hermético de
    test_appimage_failure_is_fatal, que cubre el caso opuesto (tooling presente
    pero appimagetool con rc != 0 -> fatal), y cierra la rama de degradación."""
    import build_linux

    dist = tmp_path / "dist"
    build = tmp_path / "build"
    dist.mkdir()
    build.mkdir()
    onedir = dist / "ai-voice-interconnector"
    onedir.mkdir()
    (onedir / "ai-voice-interconnector").write_text("bin", encoding="utf-8")

    monkeypatch.setattr(build_linux, "DIST_DIR", dist)
    monkeypatch.setattr(build_linux, "BUILD_DIR", build)
    monkeypatch.setattr(build_linux, "run_pyinstaller", lambda args, timeout: 0)
    monkeypatch.setattr(build_linux, "bundle_size_mb", lambda o: 0.0)
    monkeypatch.setattr(build_linux, "copy_license_files", lambda d: None)
    monkeypatch.setattr(build_linux, "ensure_png_icon", lambda p: p)
    monkeypatch.setattr(build_linux, "get_version", lambda: "9.9.9")
    monkeypatch.setattr(build_linux, "check_pyinstaller", lambda *a, **k: None)
    monkeypatch.setattr(build_linux, "check_sounddevice", lambda *a, **k: None)
    monkeypatch.setattr(build_linux, "ensure_runtime_dependencies", lambda *a, **k: None)
    # El tooling del AppImage no se pudo provisionar: el stage degrada, sin abortar.
    monkeypatch.setattr(build_linux, "provision_appimage_tooling", lambda arch: None)

    logs = []
    monkeypatch.setattr(build_linux, "log", lambda msg, *a, **k: logs.append(msg))

    # No debe elevar SystemExit: la ausencia del tooling degrada, no aborta.
    build_linux.build_linux("x86_64")
    # Y debe haber registrado el warning de degradación del stage AppImage.
    assert any("AppImage" in m and "WARNING" in m for m in logs)


class TestHostGlibcFloor:
    """La verificación de glibc del host aborta el build si el host supera el
    piso documentado (GLIBC_FLOOR), y continúa si está dentro del piso o no se
    puede medir."""

    def test_host_above_floor_aborts(self, monkeypatch):
        import build_linux

        monkeypatch.setattr(build_linux, "get_host_glibc_version", lambda: (2, 39))
        with pytest.raises(SystemExit) as exc:
            build_linux.check_host_glibc_floor()
        assert exc.value.code == 1

    def test_host_at_floor_continues(self, monkeypatch):
        import build_linux

        monkeypatch.setattr(build_linux, "get_host_glibc_version", lambda: (2, 35))
        # No debe lanzar SystemExit.
        build_linux.check_host_glibc_floor()

    def test_host_below_floor_continues(self, monkeypatch):
        import build_linux

        monkeypatch.setattr(build_linux, "get_host_glibc_version", lambda: (2, 31))
        build_linux.check_host_glibc_floor()

    def test_unmeasurable_host_continues(self, monkeypatch):
        import build_linux

        monkeypatch.setattr(build_linux, "get_host_glibc_version", lambda: None)
        build_linux.check_host_glibc_floor()