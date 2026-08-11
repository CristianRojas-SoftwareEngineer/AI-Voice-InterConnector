"""Tests estáticos del script Inno Setup generado para el instalador de Windows."""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
from create_installer_windows import generate_iss


@pytest.fixture
def iss(tmp_path):
    """ISS generado con rutas temporales y versión sintética."""
    source_dir = tmp_path / "onedir"
    source_dir.mkdir()
    output_dir = tmp_path / "out"
    output_dir.mkdir()
    info_after = tmp_path / "info.txt"
    info_after.write_text("info", encoding="utf-8")
    return generate_iss(source_dir, output_dir, "9.9.9", info_after)


def test_installer_name_includes_version_and_suffix(iss):
    # A-03: vocabulario de arquitectura unificado al estilo `uname -m` (x86_64),
    # en paridad con los AppImage de Linux.
    assert "OutputBaseFilename=tts-sidecar-9.9.9-x86_64-setup" in iss


def test_setup_uses_safe_compression(iss):
    # El .iss debe emitir compresión con output periódico visible (heartbeat de
    # CI): lzma/normal + SolidCompression=no comprimen por archivo en vez del
    # lzma2/max silencioso default que CircleCI mataba (pipelines #29-#30).
    assert "Compression=lzma/normal" in iss
    assert "SolidCompression=no" in iss
    assert "LZMAUseSeparateProcess=yes" in iss


def test_setup_postinstall_persists_console(iss):
    # W-03: el setup post-instalación se lanza vía `cmd /k` para que la consola
    # quede abierta mostrando el resultado (éxito o fallo) hasta que el usuario
    # la cierre — paridad con la Terminal persistente del .command de macOS.
    assert 'Filename: {cmd}; Parameters: "/k ""{app}\\tts-sidecar.exe"" setup"' in iss
    assert "postinstall skipifsilent runasoriginaluser nowait" in iss


def test_adds_path_conditioned_on_needsaddpath(iss):
    assert "function NeedsAddPath(Param: string): boolean;" in iss
    assert "Check: NeedsAddPath(ExpandConstant('{app}'))" in iss


def test_uninstall_trims_path_entry(iss):
    # W-01: el desinstalador debe revertir la entrada {app} del PATH de usuario (HKCU).
    assert "procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);" in iss
    assert "usUninstall" in iss
    assert "RegWriteExpandStringValue(HKCU, 'Environment', 'Path', OrigPath);" in iss


def test_uninstall_path_trim_logic_is_anchored(iss):
    """Ancla línea a línea la lógica Pascal de recorte del PATH (lo más
    cercano a un test unitario sin ejecutar Pascal): guard del paso de
    desinstalación, búsqueda envuelta en ';' case-insensitive, no-op sin match
    y las dos ramas de recorte (entrada al inicio vs. en medio/al final)."""
    assert "if CurUninstallStep <> usUninstall then" in iss
    assert (
        "P := Pos(';' + Uppercase(AppDir) + ';', ';' + Uppercase(OrigPath) + ';');"
        in iss
    )
    assert "if P = 0 then" in iss
    # Rama inicio: recorta 'AppDir;' desde el comienzo (sin separador previo).
    assert "Delete(OrigPath, 1, Length(AppDir) + 1)" in iss
    # Rama medio/fin: recorta ';AppDir' incluyendo su separador previo.
    assert "Delete(OrigPath, P - 1, Length(AppDir) + 1);" in iss


def test_installer_is_per_user_no_admin(iss):
    # Instalación per-user: sin UAC (lowest) y bajo el perfil del usuario,
    # patrón {localappdata}\Programs convencional (p.ej. VS Code).
    assert "PrivilegesRequired=lowest" in iss
    assert "DefaultDirName={localappdata}\\Programs\\tts-sidecar" in iss
    assert "PrivilegesRequired=admin" not in iss


def test_path_registry_uses_hkcu_environment(iss):
    # El PATH se escribe en el registro del usuario (HKCU\Environment), no en HKLM.
    assert 'Root: HKCU; Subkey: "Environment";' in iss


def test_no_per_machine_registry_remnant(iss):
    # Ninguna referencia residual al PATH per-machine de HKLM debe sobrevivir
    # a la migración per-user (ni en [Registry] ni en [Code]).
    assert "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment" not in iss
    assert "HKLM" not in iss


def test_without_key_manual_uninstall(iss):
    # W-02: Inno Setup genera su propia entrada ({AppId}_is1); la clave manual
    # duplicaría el programa en «Aplicaciones y características».
    assert "CurrentVersion\\Uninstall\\tts-sidecar" not in iss


def test_appid_roundtrips_to_single_brace_uninstall_key(iss):
    # W-04: Inno Setup solo escapa '{' con '{{', de modo que el AppId debe
    # desescapar al literal {E8A1B2C3-...} de una sola llave final para que la
    # clave de desinstalación {AppId}_is1 coincida con la que busca
    # 'setup --uninstall' en cli.py (regresión: AppId={{...}} generaba la clave
    # {E8A1B2C3-...}}_is1 y rompía la desinstalación por CLI).
    app_id_literal = "{E8A1B2C3-D4F5-6789-ABCD-EF0123456789}"
    assert f"AppId={{{app_id_literal}" in iss
    assert f"AppId={{{app_id_literal}}}" not in iss


def test_info_after_offers_gplv3_source_code():
    """La página InfoAfter del instalador debe ofrecer el código fuente
    bajo GPLv3 y enlazar al repositorio (GPLv3 §6)."""
    from create_installer_windows import info_after_text

    text = info_after_text()
    assert "GPLv3" in text or "GPL-3.0" in text
    assert "github.com/CristianRojas-SoftwareEngineer/TTS-Sidecar" in text
    # Debe seguir explicando la provisión del modelo (compatibilidad con W-03).
    assert "tts-sidecar setup" in text
    # El instalador NO incluye el código fuente junto al programa; la
    # oferta GPLv3 §6d correcta es la disponibilidad pública en el repositorio.
    assert "accompanido" not in text
    assert "LICENSE.txt" not in text
    assert "disponible públicamente" in text


def test_main_builds_installer_with_mocked_iscc(tmp_path, monkeypatch):
    """main() debe llegar a invocar ISCC con un .iss válido. La regresión
    de 8a18fad dejó el bloque de compilación inalcanzable dentro de
    info_after_text(): main() retornaba tras mkdir sin compilar nada, y ningún
    test lo detectaba porque solo se ejercitaban funciones puras."""
    import create_installer_windows as ciw

    onedir = tmp_path / "onedir"
    onedir.mkdir()
    (onedir / "tts-sidecar.exe").write_bytes(b"MZ")
    output_dir = tmp_path / "out"

    fake_iscc = str(tmp_path / "ISCC.exe")
    invocations = []
    iss_contents = []

    def fake_run(cmd, **kwargs):
        invocations.append(cmd)
        # ISCC ahora hereda la consola: no debe pasarse capture_output.
        assert "capture_output" not in kwargs
        # El .iss es un tempfile que main() borra en su finally: se lee aquí.
        iss_contents.append(Path(cmd[1]).read_text(encoding="utf-8"))

        class Result:
            returncode = 0

        # Simula el artefacto que ISCC dejaría en OutputDir.
        version = ciw.get_version()
        output_dir.mkdir(parents=True, exist_ok=True)
        (output_dir / f"tts-sidecar-{version}-x86_64-setup.exe").write_bytes(b"MZ")
        return Result()

    monkeypatch.setattr(
        sys, "argv",
        ["create_installer_windows.py", str(onedir), "--output", str(output_dir)],
    )
    monkeypatch.setattr(ciw, "get_inno_setup_path", lambda: fake_iscc)
    monkeypatch.setattr(ciw, "ensure_build_dependency", lambda *a, **k: True)
    monkeypatch.setattr(ciw, "ensure_ico", lambda _dir: None)
    monkeypatch.setattr(ciw.subprocess, "run", fake_run)

    ciw.main()

    # Si main() retorna sin invocar ISCC, la regresión ha vuelto.
    assert len(invocations) == 1
    cmd = invocations[0]
    assert cmd[0] == fake_iscc
    assert cmd[1].endswith(".iss")
    assert "OutputBaseFilename=tts-sidecar-" in iss_contents[0]


def test_main_installer_timeout_is_fatal(tmp_path, monkeypatch):
    """Un ISCC que excede el timeout debe abortar con SystemExit(1), no degradar."""
    import create_installer_windows as ciw

    onedir = tmp_path / "onedir"
    onedir.mkdir()
    (onedir / "tts-sidecar.exe").write_bytes(b"MZ")
    output_dir = tmp_path / "out"
    fake_iscc = str(tmp_path / "ISCC.exe")

    def fake_run(cmd, **kwargs):
        raise ciw.subprocess.TimeoutExpired(cmd=cmd, timeout=kwargs.get("timeout"))

    monkeypatch.setattr(
        sys, "argv",
        ["create_installer_windows.py", str(onedir), "--output", str(output_dir)],
    )
    monkeypatch.setattr(ciw, "get_inno_setup_path", lambda: fake_iscc)
    monkeypatch.setattr(ciw, "ensure_build_dependency", lambda *a, **k: True)
    monkeypatch.setattr(ciw, "ensure_ico", lambda _dir: None)
    monkeypatch.setattr(ciw.subprocess, "run", fake_run)

    with pytest.raises(SystemExit) as exc:
        ciw.main()
    assert exc.value.code == 1


def test_main_inno_missing_is_fatal(tmp_path, monkeypatch):
    """Inno Setup ausente (get_inno_setup_path -> None) con required=True debe
    abortar main() con SystemExit(1). El aborto lo gobierna
    ensure_build_dependency (required=True en create_installer_windows.py), no
    un sys.exit manual del llamador: este test deja sin mockear
    ensure_build_dependency para ejercitar la política real y bloquear cualquier
    regresión que duplique un sys.exit manual o degrade Inno a opcional. Espejo
    de test_main_installer_timeout_is_fatal para la ausencia del empaquetador."""
    import create_installer_windows as ciw
    import build_utils

    onedir = tmp_path / "onedir"
    onedir.mkdir()
    (onedir / "tts-sidecar.exe").write_bytes(b"MZ")
    output_dir = tmp_path / "out"

    monkeypatch.setattr(
        sys, "argv",
        ["create_installer_windows.py", str(onedir), "--output", str(output_dir)],
    )
    monkeypatch.setattr(ciw, "get_inno_setup_path", lambda: None)
    # Sin TTY (CI): ensure_build_dependency no pregunta, aplica la criticidad
    # required=True y aborta. No se mockea ensure_build_dependency.
    monkeypatch.setattr(build_utils.sys.stdin, "isatty", lambda: False)
    monkeypatch.setattr(ciw, "ensure_ico", lambda _dir: None)

    with pytest.raises(SystemExit) as exc:
        ciw.main()
    assert exc.value.code == 1
