"""Tests de las utilidades compartidas de los scripts de build."""

import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
import build_utils
import pyinstaller_wrapper
from build_utils import (
    get_version, bundle_size_mb, ensure_build_dependency, fetch_pinned_asset,
    run_pyinstaller, INSTALLER_TIMEOUT, common_pyinstaller_args,
)


def test_reads_version_from_synthetic_init(tmp_path):
    init = tmp_path / "__init__.py"
    init.write_text('__version__ = "1.2.3"\n', encoding="utf-8")
    assert get_version(init) == "1.2.3"


def test_accepts_single_quotes(tmp_path):
    init = tmp_path / "__init__.py"
    init.write_text("__version__ = '4.5.6'\n", encoding="utf-8")
    assert get_version(init) == "4.5.6"


def test_without_version_raises_runtime_error(tmp_path):
    init = tmp_path / "__init__.py"
    init.write_text('"""Módulo sin versión."""\n', encoding="utf-8")
    with pytest.raises(RuntimeError):
        get_version(init)


def test_default_reads_real_repo_version():
    from ai_voice_interconnector import __version__
    assert get_version() == __version__


class TestEnsureBuildDependency:
    """Política interactiva única de dependencias de build (verificar → avisar →
    preguntar solo con TTY → instalar pineado o degradar/abortar por criticidad)."""

    def test_present_does_not_prompt_or_install(self, monkeypatch):
        preguntas = []
        monkeypatch.setattr("builtins.input", lambda *a: preguntas.append(a) or "s")
        installations = []
        monkeypatch.setattr(build_utils.subprocess, "run", lambda *a, **k: installations.append(a))

        assert ensure_build_dependency("herramienta", lambda: True, ["pip", "install", "x"]) is True
        assert preguntas == []
        assert installations == []

    def test_absent_with_confirmation_installs_and_reverifies(self, monkeypatch):
        monkeypatch.setattr(build_utils.sys.stdin, "isatty", lambda: True)
        monkeypatch.setattr("builtins.input", lambda *a: "s")
        installations = []
        estados = iter([False, True])  # ausente antes, presente tras instalar
        monkeypatch.setattr(
            build_utils.subprocess, "run",
            lambda cmd, **k: installations.append(cmd),
        )

        result = ensure_build_dependency(
            "herramienta", lambda: next(estados), ["pip", "install", "x==1.0"],
        )

        assert result is True
        assert installations == [["pip", "install", "x==1.0"]]

    def test_absent_with_required_rejection_aborts(self, monkeypatch):
        monkeypatch.setattr(build_utils.sys.stdin, "isatty", lambda: True)
        monkeypatch.setattr("builtins.input", lambda *a: "n")
        installations = []
        monkeypatch.setattr(build_utils.subprocess, "run", lambda *a, **k: installations.append(a))

        with pytest.raises(SystemExit):
            ensure_build_dependency(
                "herramienta", lambda: False, ["pip", "install", "x"], required=True,
            )
        assert installations == []

    def test_absent_with_optional_rejection_returns_false(self, monkeypatch):
        monkeypatch.setattr(build_utils.sys.stdin, "isatty", lambda: True)
        monkeypatch.setattr("builtins.input", lambda *a: "n")
        installations = []
        monkeypatch.setattr(build_utils.subprocess, "run", lambda *a, **k: installations.append(a))

        result = ensure_build_dependency(
            "herramienta", lambda: False, ["pip", "install", "x"], required=False,
        )
        assert result is False
        assert installations == []

    def test_absent_without_tty_does_not_prompt_and_resolves_by_criticality(self, monkeypatch, capsys):
        monkeypatch.setattr(build_utils.sys.stdin, "isatty", lambda: False)

        def _input_prohibido(*a):
            raise AssertionError("no debe preguntar sin TTY")

        monkeypatch.setattr("builtins.input", _input_prohibido)
        installations = []
        monkeypatch.setattr(build_utils.subprocess, "run", lambda *a, **k: installations.append(a))

        result = ensure_build_dependency(
            "herramienta", lambda: False, ["pip", "install", "x"], required=False,
        )
        assert result is False
        assert installations == []
        assert "Instalación manual: pip install x" in capsys.readouterr().out

        with pytest.raises(SystemExit):
            ensure_build_dependency(
                "herramienta", lambda: False, ["pip", "install", "x"], required=True,
            )


class TestFetchPinnedAsset:
    """L-03: descarga pineada por URL + SHA-256 del tooling del AppImage
    (appimagetool y runtime estático). Sin red: se sirve un archivo local
    vía file:// y se verifica la rama de caché y la de checksum."""

    CONTENIDO = b"binario pineado de prueba"
    # SHA-256 precomputado de CONTENIDO.
    SHA_OK = __import__("hashlib").sha256(CONTENIDO).hexdigest()

    def _servir(self, tmp_path) -> str:
        src = tmp_path / "asset.bin"
        src.write_bytes(self.CONTENIDO)
        return src.resolve().as_uri()

    def test_downloads_and_verifies_checksum(self, tmp_path):
        url = self._servir(tmp_path)
        dest = tmp_path / "cache" / "asset.bin"

        result = fetch_pinned_asset(url, self.SHA_OK, dest)

        assert result == dest
        assert dest.read_bytes() == self.CONTENIDO

    def test_incorrect_checksum_aborts_and_removes_file(self, tmp_path):
        url = self._servir(tmp_path)
        dest = tmp_path / "cache" / "asset.bin"

        with pytest.raises(SystemExit):
            fetch_pinned_asset(url, "0" * 64, dest)
        assert not dest.exists()

    def test_cache_with_valid_checksum_does_not_download(self, tmp_path, monkeypatch):
        dest = tmp_path / "cache" / "asset.bin"
        dest.parent.mkdir(parents=True)
        dest.write_bytes(self.CONTENIDO)

        import urllib.request

        def _red_prohibida(*a, **k):
            raise AssertionError("no debe descargar si la caché tiene el hash pineado")

        monkeypatch.setattr(urllib.request, "urlopen", _red_prohibida)

        result = fetch_pinned_asset("https://example.invalid/asset.bin", self.SHA_OK, dest)
        assert result == dest

    def test_network_timeout_propagates_without_partial_artifact(self, tmp_path, monkeypatch):
        """Un timeout de red al descargar propaga la excepción (el build
        aborta) y no deja un dest parcial que la rama de caché confunda después."""
        import urllib.request

        def _expira(*a, **k):
            raise TimeoutError("timed out")

        monkeypatch.setattr(urllib.request, "urlopen", _expira)
        dest = tmp_path / "asset.bin"

        with pytest.raises(TimeoutError):
            fetch_pinned_asset("https://example.invalid/asset.bin", self.SHA_OK, dest)
        assert not dest.exists()


def test_bundle_size_mb_sums_nested_files(tmp_path):
    (tmp_path / "a.bin").write_bytes(b"x" * 1024)
    subdir = tmp_path / "sub"
    subdir.mkdir()
    (subdir / "b.bin").write_bytes(b"y" * 1024)

    esperado_mb = (1024 + 1024) / (1024 * 1024)
    assert bundle_size_mb(tmp_path) == pytest.approx(esperado_mb)


def _make_fake_popen(captured, returncode=0, timeout_on_first_wait=False):
    """Fabrica un Popen falso que registra su cmd/kwargs y simula wait()."""
    estado = {"waits": 0}

    class FakePopen:
        def __init__(self, cmd, **kwargs):
            captured.append({"cmd": cmd, "kwargs": kwargs})
            self.pid = 4242

        def wait(self, timeout=None):
            estado["waits"] += 1
            if timeout_on_first_wait and estado["waits"] == 1:
                raise subprocess.TimeoutExpired(cmd="pyinstaller", timeout=timeout)
            return returncode

    return FakePopen


class TestRunPyinstaller:
    """Lanzador común de PyInstaller: reescritura al wrapper COM en Windows,
    paso directo fuera de Windows y kill de árbol en timeout."""

    ARGS = ["/py", "-m", "PyInstaller", "--onedir", "--name", "ai-voice-interconnector"]

    def test_non_win32_passes_args_intact(self, monkeypatch):
        captured = []
        monkeypatch.setattr(build_utils.sys, "platform", "linux")
        monkeypatch.setattr(build_utils.subprocess, "Popen", _make_fake_popen(captured))

        rc = run_pyinstaller(self.ARGS, 1800)

        assert rc == 0
        assert captured[0]["cmd"] == self.ARGS
        # Grupo de procesos propio para poder matar el árbol en timeout.
        assert captured[0]["kwargs"].get("start_new_session") is True

    def test_win32_rewrites_to_wrapper_omitting_prefix(self, monkeypatch):
        captured = []
        monkeypatch.setattr(build_utils.sys, "platform", "win32")
        monkeypatch.setattr(build_utils.subprocess, "Popen", _make_fake_popen(captured))

        rc = run_pyinstaller(self.ARGS, 1800)

        assert rc == 0
        cmd = captured[0]["cmd"]
        assert cmd[0] == build_utils.sys.executable
        assert cmd[1].endswith("pyinstaller_wrapper.py")
        # El prefijo de 3 ([py, -m, PyInstaller]) se omite: solo van los args de PyInstaller.
        assert cmd[2:] == ["--onedir", "--name", "ai-voice-interconnector"]
        assert "PyInstaller" not in cmd[2:]

    def test_timeout_kills_tree_and_returns_1(self, monkeypatch):
        captured = []
        matados = []
        monkeypatch.setattr(build_utils.sys, "platform", "linux")
        monkeypatch.setattr(
            build_utils.subprocess, "Popen",
            _make_fake_popen(captured, timeout_on_first_wait=True),
        )
        monkeypatch.setattr(build_utils, "kill_process_tree", lambda pid: matados.append(pid))

        rc = run_pyinstaller(self.ARGS, 5)

        assert rc == 1
        assert matados == [4242]


class TestPyinstallerWrapper:
    """scripts/pyinstaller_wrapper.py: el bootstrap que evita el cuelgue COM
    del build Windows (fija sys.coinit_flags = 0x8 antes de importar comtypes y
    sale con os._exit para saltar el CoUninitialize() de atexit) y su main()."""

    def test_bootstrap_sets_coinit_flags_before_import_and_exits(self):
        bootstrap = pyinstaller_wrapper._BOOTSTRAP
        # COINIT_MULTITHREADED = 0x8; en el bootstrap se interpola como entero ("8").
        assert f"sys.coinit_flags = {pyinstaller_wrapper.COINIT_MULTITHREADED}" in bootstrap
        assert "os._exit" in bootstrap
        # La fijación de coinit_flags debe aparecer ANTES del import de PyInstaller
        # (que arrastra pycaw -> comtypes), pues coinit_flags no se hereda a
        # subprocesos y debe estar seteado en el propio proceso de análisis.
        idx_coinit = bootstrap.index(f"sys.coinit_flags = {pyinstaller_wrapper.COINIT_MULTITHREADED}")
        idx_import = bootstrap.index("from PyInstaller.__main__ import run")
        assert idx_coinit < idx_import

    def test_main_propagates_returncode_and_cleans_up(self, monkeypatch):
        written_paths = []
        unlinked = []
        captured = {}

        class FakeNamedTemporaryFile:
            def __init__(self, *a, **k):
                self.name = "fake_bootstrap.py"
                written_paths.append(self.name)

            def __enter__(self):
                return self

            def __exit__(self, *exc):
                return False

            def write(self, text):
                pass

        class FakeProc:
            def __init__(self, cmd, **kwargs):
                captured["cmd"] = cmd
                self.returncode = 7

            def wait(self, timeout=None):
                return self.returncode

        def fake_os_exit(code):
            captured["exit_code"] = code

        def fake_unlink(path):
            unlinked.append(path)

        monkeypatch.setattr(sys, "argv", ["pyinstaller_wrapper.py", "--onedir", "--name", "ai-voice-interconnector"])
        monkeypatch.setattr(pyinstaller_wrapper.tempfile, "NamedTemporaryFile", FakeNamedTemporaryFile)
        monkeypatch.setattr(pyinstaller_wrapper.subprocess, "Popen", FakeProc)
        monkeypatch.setattr(pyinstaller_wrapper.os, "_exit", fake_os_exit)
        monkeypatch.setattr(pyinstaller_wrapper.os, "unlink", fake_unlink)

        pyinstaller_wrapper.main()

        # Propaga el returncode real del bootstrap vía os._exit.
        assert captured["exit_code"] == 7
        # Pasa sys.argv[1:] (los args de PyInstaller) al bootstrap temporal.
        assert captured["cmd"][0] == sys.executable
        assert captured["cmd"][1] == "fake_bootstrap.py"
        assert captured["cmd"][2:] == ["--onedir", "--name", "ai-voice-interconnector"]
        # Elimina el archivo temporal tras la ejecución.
        assert unlinked == written_paths == ["fake_bootstrap.py"]


def test_installer_timeout_defined():
    """INSTALLER_TIMEOUT existe con el valor holgado para ISCC sobre ~1.6 GB."""
    assert INSTALLER_TIMEOUT == 1800


def test_common_pyinstaller_args_includes_noupx(tmp_path):
    """--noupx debe estar presente para los tres scripts de build (endurecimiento
    contra la heurística de antivirus que penaliza binarios comprimidos con UPX)."""
    args = common_pyinstaller_args(
        entry_point=tmp_path / "entry.py",
        project_root=tmp_path,
        dist_dir=tmp_path / "dist",
        build_dir=tmp_path / "build",
        data_sep=":",
    )
    assert "--noupx" in args


def test_common_pyinstaller_args_includes_ctranslate2(tmp_path):
    """--collect-all ctranslate2 debe estar presente: PyInstaller no recolecta
    automáticamente la librería nativa del runtime de traducción (Tarea 1)."""
    args = common_pyinstaller_args(
        entry_point=tmp_path / "entry.py",
        project_root=tmp_path,
        dist_dir=tmp_path / "dist",
        build_dir=tmp_path / "build",
        data_sep=":",
    )
    assert "--collect-all" in args and "ctranslate2" in args


def test_common_pyinstaller_args_includes_miniaudio(tmp_path):
    """--collect-all miniaudio debe estar presente: PyInstaller no recolecta
    automáticamente el nativo CFFI del backend de captura (Fase 4 STT)."""
    args = common_pyinstaller_args(
        entry_point=tmp_path / "entry.py",
        project_root=tmp_path,
        dist_dir=tmp_path / "dist",
        build_dir=tmp_path / "build",
        data_sep=":",
    )
    assert "--collect-all" in args and "miniaudio" in args


def test_linux_cpu_lock_contains_no_nvidia_packages():
    """El AppImage x86_64 debe quedar libre del stack CUDA."""
    repo_root = Path(__file__).resolve().parent.parent
    lock_path = repo_root / "requirements-lock-linux-cpu.txt"
    assert lock_path.exists(), "requirements-lock-linux-cpu.txt no existe"

    lineas_nvidia = [
        linea
        for linea in lock_path.read_text(encoding="utf-8").splitlines()
        if linea.lower().startswith("nvidia-")
    ]
    assert not lineas_nvidia, f"paquetes nvidia-* en el lock CPU-only: {lineas_nvidia}"
