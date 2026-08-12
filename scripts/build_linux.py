#!/usr/bin/env python3
"""
Build script for Linux x64 and ARM64 using PyInstaller --onedir.
Produces a folder with the executable + all dependencies, then wraps it
in an AppImage for distribution.
"""

import os
import re
import shutil
import sys
import subprocess
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent
DIST_DIR = PROJECT_ROOT / "dist"
BUILD_DIR = PROJECT_ROOT / "build"

sys.path.insert(0, str(Path(__file__).parent))
from build_utils import (
    log, StageTimer, BuildTimer, copy_license_files, get_version,
    check_pyinstaller, common_pyinstaller_args, bundle_size_mb, run_pyinstaller,
    ensure_png_icon, check_sounddevice,
    install_lockfile_dependencies, fetch_pinned_asset, APPIMAGE_TOOLING,
    BUILD_SUBPROCESS_TIMEOUT, PYINSTALLER_TIMEOUT, GLIBC_FLOOR,
)

# Mapea la arquitectura del CLI (--arch) a la clave del tooling de upstream de
# AppImage. AppImage impone el nombre "aarch64" para sus assets
# (appimagetool-aarch64.AppImage / runtime-aarch64), por lo que aquí se conserva
# "aarch64" como clave de APPIMAGE_TOOLING. Solo resuelve el tooling pineado:
# el nombre del artefacto propio usa target_arch directamente (arm64/x86_64,
# unificado con el flag --arch y con macOS/Windows). Ver docs/BUILD.md §2.
APPIMAGE_TOOLING_ARCH = {"x86_64": "x86_64", "arm64": "aarch64"}


def provision_appimage_tooling(appimage_arch):
    """Provisiona el tooling pineado del AppImage (appimagetool + runtime estático).

    Descarga (o reutiliza de la caché en build/) los binarios pineados por URL y
    SHA-256 en APPIMAGE_TOOLING. Es tooling del empaquetador (opcional): ante un
    fallo de red devuelve None y el llamador degrada con warning (el onedir sigue
    siendo usable). Un checksum que no coincide sí aborta (fetch_pinned_asset):
    un binario externo alterado nunca debe entrar al pipeline.
    """
    import urllib.error

    tooling = APPIMAGE_TOOLING[appimage_arch]
    cache_dir = BUILD_DIR / "appimage-tooling"
    try:
        tool_url, tool_sha = tooling["appimagetool"]
        appimagetool = fetch_pinned_asset(
            tool_url, tool_sha, cache_dir / f"appimagetool-{appimage_arch}.AppImage")
        runtime_url, runtime_sha = tooling["runtime"]
        runtime = fetch_pinned_asset(
            runtime_url, runtime_sha, cache_dir / f"runtime-{appimage_arch}")
    except (urllib.error.URLError, OSError) as exc:
        log(f"WARNING: no se pudo descargar el tooling del AppImage ({exc})")
        return None
    os.chmod(appimagetool, 0o755)
    return appimagetool, runtime


def _apprun_script() -> str:
    """Contenido del entry point AppRun del AppImage.

    El runtime del AppImage ejecuta este script; se limita a localizar el
    directorio real del AppDir (resolviendo symlinks) y delegar en el ejecutable
    del bundle onedir con los argumentos tal cual. Pura función (sin I/O) para
    poder testearla desde tests/test_build_linux.py.
    """
    return (
        '#!/bin/sh\n'
        'HERE="$(dirname "$(readlink -f "$0")")"\n'
        'exec "$HERE/usr/bin/ai-voice-interconnector" "$@"\n'
    )


def _desktop_entry() -> str:
    """Contenido del archivo .desktop del AppImage (estándar freedesktop).

    Appimagetool exige este archivo en la raíz del AppDir; se replica en
    usr/share/applications/ para la convención del estándar. El Terminal=true
    mantiene visible la salida del CLI (paridad con el .command de macOS).
    """
    return (
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Name=ai-voice-interconnector\n"
        "Exec=ai-voice-interconnector\n"
        "Icon=ai-voice-interconnector\n"
        "Categories=Utility;\n"
        "Terminal=true\n"
    )


def ensure_runtime_dependencies(target_arch="x86_64"):
    """Instala las dependencias runtime desde el lockfile (requerido para builds reproducibles).

    En x86_64 usa requirements-lock-linux-cpu.txt (CPU-only, sin paquetes nvidia-*).
    En arm64 usa requirements-lock.txt (universal, los wheels CPU-only se resuelven
    automáticamente por platform_machine == 'arm64' en el lockfile). Solo resuelve
    el lockfile correcto por arquitectura; la instalación en sí (existencia, pip
    --require-hashes, manejo de timeout/fallo) vive en
    build_utils.install_lockfile_dependencies, fuente única compartida con
    build_windows.py y build_macos.py.
    """
    if target_arch == "x86_64":
        lockfile = PROJECT_ROOT / "requirements-lock-linux-cpu.txt"
    else:
        lockfile = PROJECT_ROOT / "requirements-lock.txt"
    install_lockfile_dependencies(lockfile)


def get_host_glibc_version() -> "tuple[int, int] | None":
    """Obtiene la versión (major, minor) de glibc del host vía `ldd --version`.

    La primera línea de `ldd --version` es del estilo
    «ldd (Ubuntu GLIBC 2.35-0ubuntu3.1) 2.35»; se parsea el primer par
    `major.minor`. Retorna None si `ldd` falta, falla o su salida no tiene el
    patrón esperado —en ese caso la verificación del piso no bloquea el build.
    """
    try:
        result = subprocess.run(
            ["ldd", "--version"],
            capture_output=True, text=True, timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0:
        return None
    first_line = result.stdout.splitlines()[0] if result.stdout else ""
    m = re.search(r"(\d+)\.(\d+)", first_line)
    if not m:
        return None
    return (int(m.group(1)), int(m.group(2)))


def check_host_glibc_floor() -> None:
    """Aborta el build si la glibc del host supera el piso documentado.

    El AppImage se enlaza contra la glibc del runner de build; si ese runner
    migrara a una imagen con glibc mayor a GLIBC_FLOOR, el binario resultante
    requeriría esa glibc en runtime y fallaría en los usuarios objetivo sin que
    nada lo detectara hoy. Convertimos ese riesgo en un error de build explícito:
    subir GLIBC_FLOOR es una decisión deliberada (en build_utils.py, aquí y en
    install-linux.sh a la vez), no un efecto colateral silencioso de la
    infraestructura de CI.
    """
    host = get_host_glibc_version()
    if host is None:
        # No se pudo medir la glibc del host (p. ej. no es Linux): no bloqueamos.
        return
    if host > GLIBC_FLOOR:
        log(
            "ERROR: la glibc del host de build "
            f"({host[0]}.{host[1]}) es mayor que el piso documentado "
            f"({GLIBC_FLOOR[0]}.{GLIBC_FLOOR[1]}); el AppImage resultante "
            "requeriría glibc > piso documentado: sube GLIBC_FLOOR "
            "deliberadamente o compila en la base correcta."
        )
        sys.exit(1)


def check_dependencies(target_arch="x86_64"):
    """Check required dependencies are installed."""
    check_pyinstaller()
    ensure_runtime_dependencies(target_arch)

    with StageTimer("CheckDeps", "Verificando dependencias"):
        # sounddevice es dependencia del producto (sin ella el bundle saldría
        # sin audio): required. check_sounddevice es fuente única compartida
        # con build_macos.py.
        check_sounddevice()

        # appimagetool + runtime estático empaquetan el AppImage a partir del
        # bundle onedir; es tooling del empaquetador (opcional): sin él el
        # onedir sigue siendo usable y el stage AppImage degrada con warning.
        if provision_appimage_tooling(APPIMAGE_TOOLING_ARCH.get(target_arch, "x86_64")) is None:
            log("WARNING: el stage AppImage se omitirá si el tooling sigue sin estar disponible")


def build_linux(target_arch="x86_64"):
    """Build Linux with PyInstaller --onedir and package as AppImage."""
    arch_suffix = target_arch  # sufijo de artefacto propio: arm64/x86_64 (unificado con --arch)
    appimage_arch = APPIMAGE_TOOLING_ARCH.get(target_arch, "x86_64")  # clave de tooling de upstream (conserva aarch64)

    with BuildTimer():
        with StageTimer("Setup", "Preparando entorno de build"):
            log(f"Plataforma: Linux {arch_suffix}")
            DIST_DIR.mkdir(parents=True, exist_ok=True)
            BUILD_DIR.mkdir(parents=True, exist_ok=True)
            entry_point = PROJECT_ROOT / "bin" / "ai-voice-interconnector"

        with StageTimer("PyInstaller", "Compilando con PyInstaller (9-15 min)"):
            pyinstaller_args = common_pyinstaller_args(
                entry_point, PROJECT_ROOT, DIST_DIR, BUILD_DIR,
                data_sep=":",
                extra_collect_all=["sounddevice"],
            )
            log(f"Ejecutando: pyinstaller {' '.join(pyinstaller_args[2:])}")
            # run_pyinstaller gestiona el timeout con kill de árbol de procesos
            # (build_utils); en Linux la invocación es directa (sin wrapper COM).
            try:
                returncode = run_pyinstaller(pyinstaller_args, PYINSTALLER_TIMEOUT)
            except KeyboardInterrupt:
                log("\n[CANCEL] Build cancelado por el usuario.")
                sys.exit(130)

        if returncode != 0:
            log("PyInstaller falló", returncode)
            sys.exit(1)

        onedir = DIST_DIR / "ai-voice-interconnector"
        with StageTimer("Size", "Verificando tamaño del bundle"):
            if onedir.exists():
                log(f"Tamaño del bundle: {bundle_size_mb(onedir):.1f} MB ({onedir})")

        with StageTimer("Licenses", "Empaquetando avisos de licencia"):
            copy_license_files(onedir)

        with StageTimer("AppImage", "Generando AppImage"):
            # Tooling pineado (appimagetool + runtime estático de type2-runtime,
            # L-03): el runtime estático arranca sin libfuse2, ausente por
            # defecto en distros modernas. Provisión cacheada en build/ (si
            # check_dependencies ya descargó, esto es instantáneo).
            tooling = provision_appimage_tooling(appimage_arch)
            if tooling is None:
                log("WARNING: tooling del AppImage no disponible — AppImage no "
                    "generado; el bundle onedir sigue en dist/")
                return
            appimagetool, runtime = tooling

            version = get_version()

            # Prepara el AppDir a partir del bundle onedir de PyInstaller (ya
            # autocontenido: incluye su propio Python y todas las dependencias).
            appdir = BUILD_DIR / "AppDir"
            if appdir.exists():
                shutil.rmtree(appdir)
            usr_bin = appdir / "usr" / "bin"
            usr_bin.parent.mkdir(parents=True, exist_ok=True)
            shutil.copytree(onedir, usr_bin, symlinks=True)

            # AppRun: el runtime del AppImage lo ejecuta como entry point; delega
            # en el ejecutable del bundle pasando los argumentos tal cual.
            apprun = appdir / "AppRun"
            apprun.write_text(_apprun_script(), encoding="utf-8")
            os.chmod(apprun, 0o755)

            # .desktop e icono: appimagetool los exige en la raíz del AppDir;
            # se replican en usr/share/ (ubicación estándar freedesktop).
            desktop_content = _desktop_entry()
            (appdir / "ai-voice-interconnector.desktop").write_text(desktop_content, encoding="utf-8")
            applications_dir = appdir / "usr" / "share" / "applications"
            applications_dir.mkdir(parents=True, exist_ok=True)
            (applications_dir / "ai-voice-interconnector.desktop").write_text(desktop_content, encoding="utf-8")

            # Icono del AppImage: el logo oficial del proyecto (placeholder 1×1
            # solo si la fuente del logo faltara).
            ensure_png_icon(appdir / "ai-voice-interconnector.png")
            ensure_png_icon(appdir / "usr" / "share" / "icons" / "hicolor"
                            / "256x256" / "apps" / "ai-voice-interconnector.png")

            # appimagetool escribe la ruta de salida directamente; el propio
            # appimagetool es un AppImage, así que --appimage-extract-and-run
            # evita requerir FUSE también en los executors del CI.
            generated = DIST_DIR / f"ai-voice-interconnector-{version}-{arch_suffix}.AppImage"
            env = os.environ.copy()
            env["ARCH"] = appimage_arch
            # Consola heredada (sin capture_output): el output de appimagetool es
            # el heartbeat del step de CI. Su fallo es fatal: un build sin AppImage
            # nunca debe reportar éxito (publish-release exige el artefacto).
            result = subprocess.run(
                [str(appimagetool), "--appimage-extract-and-run",
                 "--runtime-file", str(runtime),
                 str(appdir), str(generated)],
                cwd=str(PROJECT_ROOT),
                env=env,
                timeout=BUILD_SUBPROCESS_TIMEOUT,
            )
            if result.returncode != 0 or not generated.exists():
                log(f"ERROR: La generación del AppImage falló (rc={result.returncode})")
                sys.exit(1)

            log(f"AppImage creado: {generated}")


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description="Build Linux executable")
    parser.add_argument(
        "--arch", default="x86_64", choices=["x86_64", "arm64"],
        help="Target architecture",
    )
    args = parser.parse_args()
    check_host_glibc_floor()
    check_dependencies(args.arch)
    build_linux(args.arch)
