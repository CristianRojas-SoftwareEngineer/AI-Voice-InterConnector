#!/usr/bin/env python3
"""
Build script for macOS ARM64 (Apple Silicon) apps using PyInstaller --onedir.
Produces a .app bundle (macOS application) inside the --onedir folder.

Mac Intel (x86_64) no está soportado: torch>=2.3 no publica wheels macOS
x86_64, por lo que un binario Intel no puede construirse con el toolchain
actual. El artefacto se nombra por su arquitectura real (arm64).

macOS plays audio with afplay (built-in), but device enumeration
(doctor/setup/devices) uses sounddevice, so the bundle must collect it
(PortAudio binary included); pycaw is Windows-only and not needed here.
"""

import os
import platform
import shutil
import subprocess
import sys
import sysconfig
import tarfile
from pathlib import Path

PROJECT_ROOT = Path(__file__).parent.parent
DIST_DIR = PROJECT_ROOT / "dist"
BUILD_DIR = PROJECT_ROOT / "build"

sys.path.insert(0, str(Path(__file__).parent))
from build_utils import (
    log, StageTimer, BuildTimer, copy_license_files, get_version,
    check_pyinstaller, common_pyinstaller_args, bundle_size_mb, run_pyinstaller,
    ensure_icns, check_sounddevice, fetch_pinned_asset,
    install_lockfile_dependencies, BUILD_SUBPROCESS_TIMEOUT, PYINSTALLER_TIMEOUT,
    CREATE_DMG_PIN, CREATE_DMG_TOOLING,
)


def provision_create_dmg(cache_dir=None):
    """Provisiona create-dmg pineado (tarball del release + SHA-256) y devuelve
    la ruta del script ejecutable.

    create-dmg es un script de shell puro distribuido como release en GitHub:
    se descarga (o reutiliza de la caché en build/) el tarball del tag fijado
    en CREATE_DMG_TOOLING, verificado por SHA-256 vía fetch_pinned_asset —
    misma política de supply chain que appimagetool en build_linux.py, sin
    Homebrew de por medio. Dependencia dura: el stage DMG es obligatorio
    (publish-release exige el artefacto), así que un fallo de descarga aborta
    el build en vez de degradar con warning. La extracción es idempotente
    (se omite si el script ya existe extraído).
    """
    import urllib.error

    cache_dir = Path(cache_dir) if cache_dir else BUILD_DIR / "create-dmg-tooling"
    tarball_dest = cache_dir / f"create-dmg-{CREATE_DMG_PIN}.tar.gz"
    script_path = cache_dir / f"create-dmg-{CREATE_DMG_PIN}" / "create-dmg"

    try:
        tarball = fetch_pinned_asset(
            CREATE_DMG_TOOLING["url"], CREATE_DMG_TOOLING["sha256"], tarball_dest)
    except (urllib.error.URLError, OSError) as exc:
        log(f"ERROR: no se pudo descargar create-dmg pineado ({exc})")
        log(f"  URL: {CREATE_DMG_TOOLING['url']}")
        sys.exit(1)

    if not script_path.exists():
        with tarfile.open(tarball, "r:gz") as tf:
            # filter="data" neutraliza rutas absolutas/traversal del tarball
            # (y evita el DeprecationWarning de Python 3.14).
            tf.extractall(cache_dir, filter="data")
        log(f"create-dmg {CREATE_DMG_PIN} extraído en {script_path.parent}")
    os.chmod(script_path, 0o755)
    return script_path


def check_dependencies():
    """Check required dependencies are installed.

    La instalación del lockfile en sí (existencia, pip --require-hashes,
    manejo de timeout/fallo) vive en build_utils.install_lockfile_dependencies,
    fuente única compartida con build_windows.py y build_linux.py.
    Devuelve la ruta del script create-dmg provisionado (pineado por SHA-256),
    que build_macos() recibe como parámetro.
    """
    check_pyinstaller()
    install_lockfile_dependencies(PROJECT_ROOT / "requirements-lock.txt")

    with StageTimer("CheckDeps", "Verificando dependencias"):
        # sounddevice es dependencia del producto en macOS: afplay reproduce,
        # pero doctor/setup/devices enumeran dispositivos con sounddevice
        # (audio.py). Sin ella en el bundle, todo Mac congelado reportaría
        # FAIL de audio. check_sounddevice es fuente única compartida con
        # build_linux.py.
        check_sounddevice()

        # create-dmg se provisiona pineado (tarball del release + SHA-256),
        # no vía Homebrew: dependencia dura del stage DMG.
        return provision_create_dmg()


def build_macos(target_arch="arm64", create_dmg_path="create-dmg"):
    """Build macOS .app bundle with PyInstaller --onedir.

    `create_dmg_path` es la ruta del script create-dmg provisionado por
    check_dependencies() (pineado por SHA-256); el default "create-dmg"
    (resolución por PATH) solo aplica si el llamador omite el provisioning.
    """
    arch_options = {"arm64": "arm64"}
    arch_flag = arch_options.get(target_arch, "arm64")

    with BuildTimer():
        with StageTimer("Setup", "Preparando entorno de build"):
            log(f"Plataforma: macOS {arch_flag}")
            DIST_DIR.mkdir(parents=True, exist_ok=True)
            BUILD_DIR.mkdir(parents=True, exist_ok=True)
            entry_point = PROJECT_ROOT / "bin" / "ai-voice-interconnector"

        with StageTimer("PyInstaller", "Compilando con PyInstaller (9-15 min)"):
            # afplay (built-in) es el player de macOS, pero la enumeración de
            # dispositivos (doctor/setup/devices) usa sounddevice: se recolecta
            # completo para que el binario de PortAudio viaje en el bundle.
            # Python 3.13 requiere --collect-binaries python para incluir
            # libpython3.13.dylib (PYI-20036 fix).
            pyinstaller_args = common_pyinstaller_args(
                entry_point, PROJECT_ROOT, DIST_DIR, BUILD_DIR,
                data_sep=":",
                extra_collect_all=["sounddevice"],
                extra_collect_binaries=["python"],
            )
            log(f"Ejecutando: pyinstaller {' '.join(pyinstaller_args[2:])}")
            # run_pyinstaller gestiona el timeout con kill de árbol de procesos
            # (build_utils); en macOS la invocación es directa (sin wrapper COM).
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

        with StageTimer("AppBundle", "Estructurando como bundle .app"):
            # Convert: dist/ai-voice-interconnector/ → dist/ai-voice-interconnector.app/Contents/MacOS/
            app_bundle = DIST_DIR / f"ai-voice-interconnector-{arch_flag}.app"
            macos_dir = app_bundle / "Contents" / "MacOS"
            macos_dir.mkdir(parents=True, exist_ok=True)

            if onedir.exists():
                # Move the executable and _internal/ into Contents/MacOS/
                for item in onedir.iterdir():
                    dest = macos_dir / item.name
                    if dest.exists():
                        if dest.is_dir():
                            shutil.rmtree(dest)
                        else:
                            dest.unlink()
                    shutil.move(str(item), str(dest))
                shutil.rmtree(onedir)

                # El bootloader de PyInstaller, al detectar que el ejecutable vive
                # bajo un path "*.app/Contents/MacOS/" (como este bundle armado a
                # mano), busca libpython3.13.dylib en Contents/Frameworks/ —no en
                # _internal/, donde --collect-binaries python lo deja realmente—
                # y falla con "Failed to load Python shared library" (pipeline #36).
                # Se crea Frameworks como symlink a MacOS/_internal, el mismo truco
                # de cross-linking que PyInstaller usa para Resources/Frameworks en
                # bundles con Qt.
                internal_dir = macos_dir / "_internal"
                if internal_dir.exists():
                    frameworks_dir = app_bundle / "Contents" / "Frameworks"
                    frameworks_dir.symlink_to(Path("MacOS") / "_internal", target_is_directory=True)

            # Genera el .icns del logo en Contents/Resources (None si Pillow o la
            # fuente faltan; en ese caso el .app queda sin icono nativo).
            icns_path = ensure_icns(app_bundle / "Contents" / "Resources")
            icon_name = icns_path.stem if icns_path else None

            # Write Info.plist
            info_plist = app_bundle / "Contents" / "Info.plist"
            version = get_version()
            info_plist.write_text(_info_plist_content(version, icon_name), encoding="utf-8")

            # Empaqueta los avisos de licencia dentro de Contents/Resources
            copy_license_files(app_bundle / "Contents" / "Resources")
            log(f".app bundle: {app_bundle}")

        with StageTimer("DMG", "Creando .dmg"):
            dmg_path = DIST_DIR / f"ai-voice-interconnector-{get_version()}-{arch_flag}.dmg"

            # Staging del contenido del volumen: el .app + scripts de instalación y
            # desinstalación. Cada SO integra el PATH con un mecanismo distinto:
            # Windows lo hace automáticamente desde el instalador Inno Setup, Linux
            # vía `ai-voice-interconnector setup` (symlink de $APPIMAGE en ~/.local/bin), y en
            # macOS estos scripts .command son el mecanismo opt-in (symlink en
            # /usr/local/bin con sudo, más la oferta de ejecutar `setup`).
            dmg_src = DIST_DIR / "dmg_src"
            if dmg_src.exists():
                shutil.rmtree(dmg_src)
            dmg_src.mkdir(parents=True)
            staged_app = dmg_src / app_bundle.name
            shutil.copytree(app_bundle, staged_app, symlinks=True)

            install_script = dmg_src / "Instalar (PATH + modelo).command"
            install_script.write_text(_path_install_script(app_bundle.name), encoding="utf-8")
            os.chmod(install_script, 0o755)

            uninstall_script = dmg_src / "Desinstalar (quitar del PATH).command"
            uninstall_script.write_text(_path_uninstall_script(), encoding="utf-8")
            os.chmod(uninstall_script, 0o755)

            create_dmg_args = [
                str(create_dmg_path),
                "--volname", "ai-voice-interconnector",
                "--window-pos", "200", "120",
                "--icon-size", "100",
                "--icon", app_bundle.name, "150", "185",
                "--hide-extension", app_bundle.name,
                "--app-drop-link", "480", "185",
                "--format", "ULFO",
                # Los runners de CircleCI (m4pro.medium) no tienen sesión GUI activa:
                # el AppleScript que create-dmg corre contra Finder para posicionar
                # iconos/ventana cuelga con "AppleEvent timed out (-1712)" (pipeline
                # #34). --skip-jenkins omite solo ese paso cosmético (según el propio
                # script, pensado para entornos sandbox/no-GUI); el resto del .dmg
                # (--app-drop-link, formato, volicon) no depende de Finder.
                "--skip-jenkins",
            ]
            # Icono de volumen del .dmg (logo del proyecto) si el .icns está disponible.
            if icns_path and icns_path.exists():
                create_dmg_args += ["--volicon", str(icns_path)]
            create_dmg_args += [str(dmg_path), str(dmg_src)]

            # Consola heredada (sin capture_output): el output de create-dmg es el
            # heartbeat del step de CI. Su fallo es fatal: un build sin .dmg nunca
            # debe reportar éxito (publish-release exige el artefacto).
            result = subprocess.run(
                create_dmg_args,
                timeout=BUILD_SUBPROCESS_TIMEOUT,
            )
            if result.returncode != 0:
                log(f"ERROR: La creación del .dmg falló (rc={result.returncode}; "
                    f"create-dmg {CREATE_DMG_PIN} pineado en "
                    f"{CREATE_DMG_TOOLING['url']})")
                sys.exit(1)
            log(f".dmg creado: {dmg_path}")


def _path_install_script(app_name: str) -> str:
    """Genera el script de instalación del .dmg: PATH + oferta de provisión.

    Se incluye en el volumen del .dmg con doble función: enlaza ai-voice-interconnector en
    ~/.local/bin (per-user, SIN `sudo`) y a continuación ofrece ejecutar
    `ai-voice-interconnector setup` (descarga del modelo) como el usuario actual, replicando
    el checkbox post-instalación del instalador de Windows.

    Instalación per-user, sin privilegios de admin: el symlink se crea en
    `~/.local/bin`, propiedad del usuario, así que ninguna vía de instalación del
    proyecto pide ya la contraseña de administrador (paridad con el per-user de
    Windows y el `~/.local` de Linux). Como `~/.local/bin` no está en el PATH por
    defecto de zsh en macOS, el script detecta su ausencia y emite la línea exacta
    para el shell profile (mismo patrón que `cli.py::_integrate_linux_path`).
    `setup` se ejecuta SIN sudo para que la caché de HuggingFace quede en el
    perfil del usuario real.
    """
    return f"""#!/bin/bash
# Expone ai-voice-interconnector en el PATH creando un symlink per-user en ~/.local/bin
# (sin privilegios de administrador) y ofrece descargar el modelo (setup).
set -e

APP="/Applications/{app_name}"
TARGET="$APP/Contents/MacOS/ai-voice-interconnector"
LINK="$HOME/.local/bin/ai-voice-interconnector"

if [ ! -x "$TARGET" ]; then
    echo "No se encontró {app_name} en /Applications."
    echo "Arrastra primero {app_name} a la carpeta Aplicaciones y vuelve a ejecutar este script."
    exit 1
fi

mkdir -p "$HOME/.local/bin"
ln -sf "$TARGET" "$LINK"
echo "Listo: 'ai-voice-interconnector' está disponible en la terminal (via $LINK)."

case ":$PATH:" in
    *":$HOME/.local/bin:"*)
        ;;
    *)
        echo
        echo "AVISO: ~/.local/bin no está en tu PATH."
        echo "Añade esta línea a tu shell profile (~/.zshrc) y reinicia la terminal:"
        echo '    export PATH="$HOME/.local/bin:$PATH"'
        ;;
esac
echo
echo "Los modelos de voz (es-mx-latam y en, ~6 GB en total) no vienen incluidos:"
echo "se descargan una sola vez con 'ai-voice-interconnector setup'."
read -p "¿Descargar ahora el modelo de voz? (s/n): " RESPUESTA
case "$RESPUESTA" in
    [sSyY]*)
        "$TARGET" setup
        ;;
    *)
        echo "Puedes hacerlo más tarde con: ai-voice-interconnector setup"
        ;;
esac
"""


def _path_uninstall_script() -> str:
    """Genera el script de desinstalación del .dmg: revierte el symlink de PATH.

    Elimina ~/.local/bin/ai-voice-interconnector (per-user, SIN `sudo`) solo si es un symlink;
    si no existe lo informa y termina sin error, y si es un archivo regular
    homónimo lo rechaza sin tocarlo. Además detecta un symlink legado en
    /usr/local/bin (de versiones anteriores, que sí usaban `sudo`) e instruye
    cómo quitarlo manualmente. El .app se elimina arrastrándolo a la Papelera.
    """
    return """#!/bin/bash
# Quita el symlink per-user de ai-voice-interconnector de ~/.local/bin (reversión de la
# instalación), sin sudo.
set -e

LINK="$HOME/.local/bin/ai-voice-interconnector"

if [ -L "$LINK" ]; then
    rm "$LINK"
    echo "Symlink eliminado: $LINK"
    echo "Para completar la desinstalación, arrastra ai-voice-interconnector.app a la Papelera."
elif [ -e "$LINK" ]; then
    echo "$LINK existe pero no es un symlink; no se elimina."
    exit 1
else
    echo "No hay nada que quitar: $LINK no existe."
fi

# Transición desde versiones anteriores: aquellas creaban el symlink en
# /usr/local/bin con sudo. Si existe, se informa cómo quitarlo (requiere sudo).
LEGACY="/usr/local/bin/ai-voice-interconnector"
if [ -L "$LEGACY" ]; then
    echo
    echo "AVISO: se detectó un symlink legado en $LEGACY (de una versión anterior)."
    echo "Elimínalo manualmente con:"
    echo "    sudo rm $LEGACY"
fi
"""


def _minimum_system_version():
    """Deriva el mínimo de macOS soportado del propio toolchain de build.

    Un literal fijo (p.ej. "12.0") se desincroniza en silencio del SDK
    real con el que pyenv compila el CPython empaquetado — un usuario en esa
    versión declarada instalaría un bundle que en realidad no arranca en su
    sistema. `MACOSX_DEPLOYMENT_TARGET` es la fuente de verdad: es el mismo
    valor que el linker usa para fijar el `LC_VERSION_MIN_MACOSX`/`LC_BUILD_VERSION`
    del binario resultante. Si sysconfig no lo expone (build no estándar),
    se recurre a la versión mayor del macOS del host que compila.
    """
    target = sysconfig.get_config_var("MACOSX_DEPLOYMENT_TARGET")
    if target:
        return target

    mac_ver = platform.mac_ver()[0]
    if mac_ver:
        return mac_ver.split(".")[0] + ".0"

    return "12.0"


def _info_plist_content(version, icon_name=None):
    """Genera el contenido XML del Info.plist del bundle .app para macOS.

    Define los metadatos del bundle (CFBundleIdentifier, CFBundleVersion, etc.)
    que macOS usa para identificar la aplicación en el sistema. `icon_name` es el
    nombre del .icns sin extensión (convención de CFBundleIconFile); si es None,
    el campo queda vacío (Finder muestra el icono genérico).
    """
    icon_value = icon_name or ""
    min_system_version = _minimum_system_version()
    return f"""\
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleExecutable</key>
    <string>ai-voice-interconnector</string>
    <key>CFBundleIdentifier</key>
    <string>com.ai-voice-interconnector.app</string>
    <key>CFBundleName</key>
    <string>ai-voice-interconnector</string>
    <key>CFBundleDisplayName</key>
    <string>ai-voice-interconnector</string>
    <key>CFBundleVersion</key>
    <string>{version}</string>
    <key>CFBundleShortVersionString</key>
    <string>{version}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleIconFile</key>
    <string>{icon_value}</string>
    <key>LSMinimumSystemVersion</key>
    <string>{min_system_version}</string>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSHumanReadableCopyright</key>
    <string>© 2026 Cristián Rojas Arredondo — GPL-3.0-or-later</string>
    <key>NSPrincipalClass</key>
    <string>NSApplication</string>
</dict>
</plist>
"""


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser(description="Build macOS .app bundle")
    parser.add_argument(
        "--arch", default="arm64",
        choices=["arm64"],
        help="Target architecture (default: arm64; Mac Intel no está soportado)",
    )
    args = parser.parse_args()
    create_dmg_path = check_dependencies()
    build_macos(args.arch, create_dmg_path)
