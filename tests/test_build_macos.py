"""Tests estáticos de los templates generados por build_macos.py.

Aserciones de cadena sobre las funciones puras: _path_install_script,
_path_uninstall_script, _info_plist_content.
"""

import os
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
from build_macos import (
    _info_plist_content,
    _minimum_system_version,
    _path_install_script,
    _path_uninstall_script,
)


def test_info_plist_version_and_lsminimum():
    """Info.plist debe llevar la versión y el LSMinimumSystemVersion derivado del toolchain ."""
    plist = _info_plist_content("9.9.9", icon_name="ai-voice-interconnector")
    assert "<key>CFBundleShortVersionString</key>" in plist
    assert "<string>9.9.9</string>" in plist
    assert "<key>LSMinimumSystemVersion</key>" in plist
    assert f"<string>{_minimum_system_version()}</string>" in plist
    assert "GPL-3.0-or-later" in plist


def test_minimum_system_version_uses_deployment_target(monkeypatch):
    """Prioriza MACOSX_DEPLOYMENT_TARGET; cae a mac_ver() si falta."""
    import build_macos

    monkeypatch.setattr(
        build_macos.sysconfig, "get_config_var", lambda name: "14.4"
    )
    assert build_macos._minimum_system_version() == "14.4"

    monkeypatch.setattr(build_macos.sysconfig, "get_config_var", lambda name: None)
    monkeypatch.setattr(
        build_macos.platform, "mac_ver", lambda: ("15.1.1", ("", "", ""), "")
    )
    assert build_macos._minimum_system_version() == "15.0"

    monkeypatch.setattr(build_macos.platform, "mac_ver", lambda: ("", ("", "", ""), ""))
    assert build_macos._minimum_system_version() == "12.0"


def test_install_script_per_user_symlink_no_sudo_and_setup_offer():
    """El .command de instalación: symlink per-user en ~/.local/bin, SIN sudo,
    aviso de PATH y oferta de setup."""
    script = _path_install_script("ai-voice-interconnector-arm64.app")
    assert script.startswith("#!/bin/bash\n")
    assert "set -e" in script
    # Per-user, sin privilegios de admin: nada de sudo ni /usr/local/bin.
    assert "sudo" not in script
    assert "/usr/local/bin" not in script
    assert '$HOME/.local/bin/ai-voice-interconnector' in script
    assert 'mkdir -p "$HOME/.local/bin"' in script
    assert "ln -sf" in script
    # Aviso de PATH (~/.local/bin no está en el PATH por defecto de zsh en macOS).
    assert "no está en tu PATH" in script
    assert 'export PATH="$HOME/.local/bin:$PATH"' in script
    # Oferta de descargar el modelo (ai-voice-interconnector setup) en el contexto del usuario
    assert "ai-voice-interconnector setup" in script
    assert "Descargar ahora el modelo de voz" in script
    assert "s/n" in script  # prompt interactivo


def test_uninstall_script_per_user_no_sudo_and_legacy_note():
    """Desinstalación per-user: elimina el symlink de ~/.local/bin sin sudo,
    rechaza archivo regular homónimo e informa del symlink legado."""
    script = _path_uninstall_script()
    assert script.startswith("#!/bin/bash\n")
    assert "set -e" in script
    assert 'LINK="$HOME/.local/bin/ai-voice-interconnector"' in script
    assert 'if [ -L "$LINK" ]; then' in script
    assert 'elif [ -e "$LINK" ]; then' in script
    assert "no es un symlink" in script
    assert "exit 1" in script
    # La eliminación del symlink per-user no usa sudo.
    assert "rm \"$LINK\"" in script
    # Detección informativa del symlink legado en /usr/local/bin (transición).
    assert 'LEGACY="/usr/local/bin/ai-voice-interconnector"' in script
    assert "symlink legado" in script
    assert "sudo rm $LEGACY" in script


def test_create_dmg_failure_is_fatal(tmp_path, monkeypatch):
    """create-dmg con rc != 0 debe abortar el build con SystemExit(1),
    heredando la consola (sin capture_output) para que su output sea visible."""
    import build_macos

    dist = tmp_path / "dist"
    build = tmp_path / "build"
    dist.mkdir()
    build.mkdir()
    onedir = dist / "ai-voice-interconnector"
    onedir.mkdir()
    (onedir / "ai-voice-interconnector").write_text("bin", encoding="utf-8")

    monkeypatch.setattr(build_macos, "DIST_DIR", dist)
    monkeypatch.setattr(build_macos, "BUILD_DIR", build)
    monkeypatch.setattr(build_macos, "run_pyinstaller", lambda args, timeout: 0)
    monkeypatch.setattr(build_macos, "bundle_size_mb", lambda o: 0.0)
    monkeypatch.setattr(build_macos, "copy_license_files", lambda d: None)
    monkeypatch.setattr(build_macos, "ensure_icns", lambda d: None)
    monkeypatch.setattr(build_macos, "get_version", lambda: "9.9.9")

    captured = {}

    class Result:
        returncode = 1

    def fake_run(*a, **k):
        captured.update(k)
        return Result()

    monkeypatch.setattr(build_macos.subprocess, "run", fake_run)

    fake_create_dmg = tmp_path / "create-dmg"
    fake_create_dmg.write_text("x", encoding="utf-8")

    with pytest.raises(SystemExit) as exc:
        build_macos.build_macos("arm64", create_dmg_path=fake_create_dmg)
    assert exc.value.code == 1
    assert "capture_output" not in captured


class TestProvisionCreateDmg:
    """provision_create_dmg descarga el tarball pineado, lo extrae de forma
    idempotente y devuelve la ruta del script ejecutable — sin red en tests
    (fetch_pinned_asset mockeado con un tarball sintético local)."""

    @staticmethod
    def _make_tarball(tmp_path):
        """Construye un tarball sintético con la estructura del release real
        (create-dmg-<PIN>/create-dmg) y lo deja donde provision_create_dmg
        espera la descarga."""
        import tarfile
        from build_utils import CREATE_DMG_PIN

        src_root = tmp_path / "src" / f"create-dmg-{CREATE_DMG_PIN}"
        src_root.mkdir(parents=True)
        (src_root / "create-dmg").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
        tarball = tmp_path / f"create-dmg-{CREATE_DMG_PIN}.tar.gz"
        with tarfile.open(tarball, "w:gz") as tf:
            tf.add(src_root, arcname=src_root.name)
        return tarball

    def test_extracts_and_returns_script_path(self, tmp_path, monkeypatch):
        import build_macos
        from build_utils import CREATE_DMG_PIN

        tarball = self._make_tarball(tmp_path)
        cache_dir = tmp_path / "cache"
        monkeypatch.setattr(
            build_macos, "fetch_pinned_asset", lambda url, sha, dest: tarball
        )

        script = build_macos.provision_create_dmg(cache_dir)
        assert script == cache_dir / f"create-dmg-{CREATE_DMG_PIN}" / "create-dmg"
        assert script.exists()
        if sys.platform != "win32":
            assert os.access(script, os.X_OK)

    def test_extraction_is_idempotent(self, tmp_path, monkeypatch):
        import build_macos

        tarball = self._make_tarball(tmp_path)
        cache_dir = tmp_path / "cache"
        monkeypatch.setattr(
            build_macos, "fetch_pinned_asset", lambda url, sha, dest: tarball
        )

        script = build_macos.provision_create_dmg(cache_dir)
        # Segunda invocación con el script ya extraído: no debe re-extraer
        # (se marca el archivo y se verifica que sobrevive intacto).
        script.write_text("marcado", encoding="utf-8")
        script_again = build_macos.provision_create_dmg(cache_dir)
        assert script_again == script
        assert script.read_text(encoding="utf-8") == "marcado"

    def test_download_failure_aborts(self, tmp_path, monkeypatch):
        import urllib.error
        import build_macos

        def _falla(url, sha, dest):
            raise urllib.error.URLError("sin red")

        monkeypatch.setattr(build_macos, "fetch_pinned_asset", _falla)
        with pytest.raises(SystemExit) as exc:
            build_macos.provision_create_dmg(tmp_path / "cache")
        assert exc.value.code == 1