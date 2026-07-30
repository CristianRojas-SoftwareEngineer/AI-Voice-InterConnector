#!/usr/bin/env python3
"""
Build script for Windows x64 using PyInstaller --onedir.
Produces a folder with the executable + all dependencies, then wraps it
in an Inno Setup installer.
"""

import os
import sys
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent
DIST_DIR = PROJECT_ROOT / "dist"
BUILD_DIR = PROJECT_ROOT / "build"

# Import shared logging utilities
sys.path.insert(0, str(Path(__file__).parent))
from build_utils import (
    log, StageTimer, BuildTimer, copy_license_files, get_version,
    check_pyinstaller, common_pyinstaller_args, bundle_size_mb, run_pyinstaller,
    install_lockfile_dependencies,
    PYINSTALLER_TIMEOUT, INSTALLER_TIMEOUT,
)

# Plantilla del archivo de versión PE (formato pyinstaller-versionfile /
# VSVersionInfo) consumido por --version-file: da al .exe metadata de
# identidad (empresa, producto, versión) que la heurística de los antivirus
# usa como señal a favor de un binario legítimo (docs/SELF-HOSTED-INSTALL.md).
_VERSION_FILE_TEMPLATE = """\
# UTF-8
VSVersionInfo(
  ffi=FixedFileInfo(
    filevers=({v0}, {v1}, {v2}, 0),
    prodvers=({v0}, {v1}, {v2}, 0),
    mask=0x3f,
    flags=0x0,
    OS=0x40004,
    fileType=0x1,
    subtype=0x0,
    date=(0, 0),
  ),
  kids=[
    StringFileInfo(
      [
        StringTable(
          u'040904B0',
          [StringStruct(u'CompanyName', u'TTS Sidecar'),
          StringStruct(u'FileDescription', u'TTS Sidecar - motor de sintesis de voz offline'),
          StringStruct(u'FileVersion', u'{version}'),
          StringStruct(u'InternalName', u'tts-sidecar'),
          StringStruct(u'LegalCopyright', u'(C) 2026 Cristian Rojas Arredondo - GPL-3.0-or-later'),
          StringStruct(u'OriginalFilename', u'tts-sidecar.exe'),
          StringStruct(u'ProductName', u'TTS Sidecar'),
          StringStruct(u'ProductVersion', u'{version}')])
      ]),
    VarFileInfo([VarStruct(u'Translation', [1033, 1200])])
  ]
)
"""


def _write_version_file(dest_dir: Path) -> Path:
    """Genera el archivo de versión PE que consume --version-file de PyInstaller.

    Deriva los cuatro campos numéricos (filevers/prodvers) de get_version(),
    la misma fuente única de versión que usan build_macos.py (Info.plist) y
    create_installer_windows.py (Inno Setup). Los componentes no numéricos
    (p. ej. sufijos "-rc1") se descartan del versionado numérico pero la
    versión completa sigue viajando en FileVersion/ProductVersion como texto.
    """
    version = get_version()
    parts = version.split(".")
    nums = []
    for part in (parts + ["0", "0", "0"])[:3]:
        digits = "".join(ch for ch in part if ch.isdigit())
        nums.append(int(digits) if digits else 0)
    v0, v1, v2 = nums

    dest_dir.mkdir(parents=True, exist_ok=True)
    version_file = dest_dir / "version_info.txt"
    version_file.write_text(
        _VERSION_FILE_TEMPLATE.format(v0=v0, v1=v1, v2=v2, version=version),
        encoding="utf-8",
    )
    log(f"Archivo de versión PE generado: {version_file} ({version})")
    return version_file


def check_dependencies():
    """Verifica que las dependencias requeridas estén instaladas.

    La instalación del lockfile en sí (existencia, pip --require-hashes,
    manejo de timeout/fallo) vive en build_utils.install_lockfile_dependencies,
    fuente única compartida con build_linux.py y build_macos.py.
    """
    check_pyinstaller()
    install_lockfile_dependencies(PROJECT_ROOT / "requirements-lock.txt")


def build_windows(target_arch="x86_64", no_installer=False):
    """Compila tts-sidecar para Windows x64 con PyInstaller --onedir."""
    with BuildTimer():
        with StageTimer("Setup", "Preparando entorno de build"):
            log("Plataforma: Windows x64")
            DIST_DIR.mkdir(parents=True, exist_ok=True)
            BUILD_DIR.mkdir(parents=True, exist_ok=True)
            entry_point = PROJECT_ROOT / "bin" / "tts-sidecar"
            log(f"Entry point: {entry_point}")

        with StageTimer("PyInstaller", "Compiling with PyInstaller (9-15 min)"):
            pyinstaller_args = common_pyinstaller_args(
                entry_point, PROJECT_ROOT, DIST_DIR, BUILD_DIR,
                data_sep=";",
                extra_collect_all=["pycaw"],
            )
            # Metadata PE de identidad (empresa/producto/versión): solo tiene
            # sentido en Windows, el .AppImage de Linux es ELF y no la soporta.
            version_file = _write_version_file(BUILD_DIR)
            pyinstaller_args += ["--version-file", str(version_file)]
            # [2:] omite [sys.executable, "-m"] del log para mostrar solo los args de PyInstaller
            log(f"Running: pyinstaller {' '.join(pyinstaller_args[2:])}")
            # run_pyinstaller reescribe la invocación al wrapper COM en Windows y
            # gestiona el timeout con kill de árbol de procesos (build_utils).
            try:
                returncode = run_pyinstaller(pyinstaller_args, PYINSTALLER_TIMEOUT)
            except KeyboardInterrupt:
                log("\n[CANCEL] Build cancelado por el usuario.")
                sys.exit(130)  # 128 + 2 (SIGINT)

        if returncode != 0:
            log("PyInstaller falló", returncode)
            sys.exit(1)

        with StageTimer("Size", "Verificando tamaño del bundle"):
            onedir = DIST_DIR / "tts-sidecar"
            if onedir.exists():
                log(f"Tamaño del bundle: {bundle_size_mb(onedir):.1f} MB ({onedir})")

        with StageTimer("Licenses", "Empaquetando avisos de licencia"):
            copy_license_files(DIST_DIR / "tts-sidecar")

        if no_installer:
            log("Etapa Installer omitida (--no-installer); el CI la corre como step separado")
        else:
            with StageTimer("Installer", "Generando instalador Inno Setup"):
                installer_script = PROJECT_ROOT / "scripts" / "create_installer_windows.py"
                try:
                    returncode = subprocess.run(
                        [sys.executable, str(installer_script), str(DIST_DIR / "tts-sidecar")],
                        check=False,
                        # Margen sobre el timeout interno de ISCC (create_installer_windows).
                        timeout=INSTALLER_TIMEOUT + 60,
                    ).returncode
                except subprocess.TimeoutExpired:
                    log(f"[TIMEOUT] La generación del instalador excedió {INSTALLER_TIMEOUT + 60}s")
                    returncode = 1
                if returncode != 0:
                    # Fatal: un build sin instalador nunca debe reportar éxito
                    # (el step de CI/publish-release exige el .exe versionado).
                    log(f"ERROR: Generación del instalador fallida (rc={returncode})")
                    sys.exit(1)
                log("Instalador creado correctamente")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description="Build Windows executable")
    parser.add_argument(
        "--arch", default="x86_64",
        choices=["x86_64"],
        help="Target architecture (default: x86_64; Windows-on-ARM no está soportado)",
    )
    parser.add_argument(
        "--no-installer", action="store_true",
        help="Omite la etapa Installer (el CI la corre como step separado)",
    )
    args = parser.parse_args()
    check_dependencies()
    build_windows(args.arch, args.no_installer)
