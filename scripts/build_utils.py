"""
Utilidades compartidas para los scripts de build.
Provee logging con timestamp [HH:MM:SS], tracking de etapas y el empaquetado
de los avisos de licencia en el bundle distribuible.
"""

import base64
import os
import shutil
import signal
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path

# Archivos de cumplimiento de licencia que deben viajar dentro de cada artefacto
# distribuible (PyInstaller elimina los avisos de licencia de las dependencias).
# SOURCE-OFFER.md es la oferta escrita de código fuente (GPLv3 §6): acompaña al
# binario para que sobreviva a cualquier vía de redistribución.
LICENSE_FILES = ("LICENSE", "THIRD-PARTY-LICENSES.md", "SOURCE-OFFER.md")

# Fuente única del logo del proyecto (PNG 256×256), del que los tres builds
# derivan su icono nativo: PNG directo en Linux, .ico en Windows, .icns en macOS.
LOGO_SOURCE = Path(__file__).parent.parent / "assets" / "images" / "AI Voice InterConnector - Logo.png"

# PNG 1×1 transparente (base64) usado como placeholder de icono cuando la fuente
# del logo no existe. El AppImage exige un icono presente en el AppDir.
_PLACEHOLDER_PNG_1X1 = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
)

# Timeout aplicado a los subprocesos de empaquetado de plataforma (appimagetool
# en Linux, create-dmg en macOS) para que un empaquetador colgado no cuelgue el job
# de CI indefinidamente; consistente con el timeout ya usado por el instalador de
# Windows (create_installer_windows.py).
BUILD_SUBPROCESS_TIMEOUT = 600

# Timeout para el propio subprocess de PyInstaller, la etapa más larga del
# build (9-15 min típico según la plataforma): más holgado que
# BUILD_SUBPROCESS_TIMEOUT para no abortar una compilación legítima que
# simplemente tarda más en un runner de CI cargado.
PYINSTALLER_TIMEOUT = 1800

# Timeout para la generación del instalador Inno Setup (ISCC): comprimir el
# onedir de ~1.6 GB con lzma/normal tarda ~3-5 min con progreso por archivo,
# pero un runner de CI cargado puede acercarse al límite; los 600 s del timeout
# genérico eran insuficientes y provocaban un TimeoutExpired no atrapado que
# dejaba el instalador ausente (evidencia pipeline #30).
INSTALLER_TIMEOUT = 1800

# Versiones pineadas de las herramientas de build, espejo de las que instala
# .circleci/config.yml: un build local con estas versiones produce el mismo
# artefacto que el CI. Actualizar deliberadamente y en ambos lugares a la vez.
# INNOSETUP_PIN es la fuente única del pin de Inno (lado Python del diseño
# mixto de los builds de Windows): tests/test_pin_consistency.py falla si el comando
# `choco install innosetup --version=...` de config.yml diverge de esta
# constante. PYINSTALLER_PIN alimenta requirements-lock-build.txt.
PYINSTALLER_PIN = "6.21.0"
INNOSETUP_PIN = "6.3.3"

# Lockfile con hashes de la herramienta de build PyInstaller (y sus
# dependencias transitivas), separado de los lockfiles de runtime
# (requirements-lock.txt / requirements-lock-linux-cpu.txt): PyInstaller no es
# una dependencia del producto (no debe viajar en el wheel de PyPI), así que
# no pertenece a pyproject.toml ni a sus lockfiles derivados. Fija
# PYINSTALLER_PIN con --require-hashes para que la instalación del compilador
# sea tan reproducible como la de las dependencias runtime; regenerar tras
# actualizar PYINSTALLER_PIN (ver cabecera del propio archivo).
PYINSTALLER_LOCKFILE = Path(__file__).parent.parent / "requirements-lock-build.txt"

# Tooling pineado del empaquetado AppImage (L-03): appimagetool empaqueta el
# AppDir y el runtime estático de type2-runtime (FUSE 3 estático con
# autoextracción de respaldo) se incrusta con --runtime-file, eliminando la
# dependencia de libfuse2 que las distros modernas ya no instalan por defecto.
# Pines por URL de release + SHA-256 (fetch_pinned_asset los verifica): misma
# política que los pines de versión de pip/choco. Actualizar deliberadamente
# URL y hash a la vez.
APPIMAGETOOL_PIN = "1.9.1"
TYPE2_RUNTIME_PIN = "20251108"
APPIMAGE_TOOLING = {
    "x86_64": {
        "appimagetool": (
            f"https://github.com/AppImage/appimagetool/releases/download/{APPIMAGETOOL_PIN}/appimagetool-x86_64.AppImage",
            "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0",
        ),
        "runtime": (
            f"https://github.com/AppImage/type2-runtime/releases/download/{TYPE2_RUNTIME_PIN}/runtime-x86_64",
            "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
        ),
    },
    "aarch64": {
        "appimagetool": (
            f"https://github.com/AppImage/appimagetool/releases/download/{APPIMAGETOOL_PIN}/appimagetool-aarch64.AppImage",
            "f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158",
        ),
        "runtime": (
            f"https://github.com/AppImage/type2-runtime/releases/download/{TYPE2_RUNTIME_PIN}/runtime-aarch64",
            "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
        ),
    },
}

# Tooling pineado del empaquetado .dmg de macOS: create-dmg es un
# script de shell puro distribuido como release en GitHub, así que se fija por
# URL de tarball del tag + SHA-256 (fetch_pinned_asset lo verifica), igual que
# appimagetool — eliminando la instalación sin pin vía `brew install`.
# Actualizar deliberadamente URL y hash a la vez.
CREATE_DMG_PIN = "1.3.0"
CREATE_DMG_TOOLING = {
    "url": f"https://github.com/create-dmg/create-dmg/archive/refs/tags/v{CREATE_DMG_PIN}.tar.gz",
    "sha256": "c50d2bc97c3d6292642bac55f530d247eaf4bf65ee605f26b4caf339383e381c",
}

# Piso mínimo de glibc del AppImage, declarado como fuente única.
# El AppImage se enlaza contra la glibc del runner de build (base Ubuntu 22.04,
# glibc 2.35); si el runner migrara a una imagen con glibc mayor, el binario
# resultante exigiría esa glibc en runtime y fallaría en los usuarios objetivo
# sin que nada lo detectara hoy. build_linux.py verifica la glibc del host
# contra este piso (aborto fail-fast), y
# tests/test_pin_consistency.py.TestGlibcFloorConsistency vigila que
# install-linux.sh declare el mismo piso. Subirlo es una decisión deliberada en
# los tres lugares a la vez (aquí, build_linux.py, install-linux.sh).
GLIBC_FLOOR = (2, 35)


def module_available(module_name: str) -> bool:
    """Comprueba si un módulo Python es importable, sin importarlo.

    Invalida las cachés de los finders para que una instalación pip hecha en
    este mismo proceso (ensure_build_dependency) sea visible al re-verificar.
    """
    import importlib
    import importlib.util

    importlib.invalidate_caches()
    return importlib.util.find_spec(module_name) is not None


def ensure_build_dependency(name, check, install_cmd=None, required=False) -> bool:
    """Verifica una dependencia de build y, si falta, avisa y ofrece instalarla.

    Política única de los tres scripts de build (misma UX en Windows, Linux y
    macOS, sobre pip/brew/choco):

    - `check` (callable → bool) presente: no pregunta nada, retorna True.
    - Ausente con TTY: muestra el comando exacto (`install_cmd`, lista argv) y
      pregunta s/n; instala solo con confirmación y re-verifica con `check`.
    - Ausente sin TTY (CI) o sin `install_cmd`: no pregunta; emite la
      instrucción de instalación manual y resuelve según criticidad.
    - `required=True` distingue las dependencias del producto o del compilador
      (sin ellas el build no tiene sentido: aborta con sys.exit(1) si no se
      resuelven) de las herramientas del empaquetador (retorna False y el
      llamador degrada con warning).
    """
    if check():
        log(f"{name}: instalado")
        return True

    log(f"WARNING: {name} no está instalado")
    manual_cmd = " ".join(install_cmd) if install_cmd else None

    if install_cmd and sys.stdin.isatty():
        answer = input(f"¿Instalar {name} ahora con '{manual_cmd}'? (s/n): ")
        if answer.strip().lower() in ("s", "si", "sí", "y", "yes"):
            log(f"Instalando {name}: {manual_cmd}")
            subprocess.run(install_cmd, check=True, timeout=BUILD_SUBPROCESS_TIMEOUT)
            if check():
                log(f"{name}: instalado correctamente")
                return True
            log(f"WARNING: {name} sigue sin estar disponible tras la instalación")

    if manual_cmd:
        log(f"Instalación manual: {manual_cmd}")
    if required:
        log(f"ERROR: {name} es una dependencia requerida del build; abortando")
        sys.exit(1)
    return False


def fetch_pinned_asset(url: str, sha256: str, dest) -> Path:
    """Descarga un binario pineado verificando su SHA-256, con caché en dest.

    Si dest ya existe con el hash esperado, no descarga (caché). Si el hash
    del archivo descargado no coincide con el pin, elimina el archivo y aborta
    el build: un binario externo alterado nunca debe entrar al artefacto.
    Devuelve la ruta de dest.
    """
    import hashlib
    import urllib.request

    dest = Path(dest)

    def _sha256_of(path: Path) -> str:
        digest = hashlib.sha256()
        with open(path, "rb") as fh:
            for chunk in iter(lambda: fh.read(1 << 20), b""):
                digest.update(chunk)
        return digest.hexdigest()

    if dest.exists() and _sha256_of(dest) == sha256:
        log(f"{dest.name}: en caché (checksum OK)")
        return dest

    dest.parent.mkdir(parents=True, exist_ok=True)
    log(f"Descargando {url} -> {dest}")
    with urllib.request.urlopen(url, timeout=BUILD_SUBPROCESS_TIMEOUT) as resp, \
            open(dest, "wb") as out:
        shutil.copyfileobj(resp, out)

    actual = _sha256_of(dest)
    if actual != sha256:
        dest.unlink()
        log(f"ERROR: checksum de {dest.name} no coincide con el pin")
        log(f"  esperado: {sha256}")
        log(f"  obtenido: {actual}")
        sys.exit(1)
    log(f"{dest.name}: descargado y verificado (SHA-256 OK)")
    return dest


def check_pyinstaller() -> None:
    """Verifica que PyInstaller esté instalado (ofrece instalarlo si falta).

    Fuente única para los tres scripts de build: antes cada uno duplicaba este
    mismo bloque de try/except. Criticidad required: sin el
    compilador el build no tiene sentido. La instalación manual/ofrecida usa
    PYINSTALLER_LOCKFILE (--require-hashes) en vez de `pip install
    pyinstaller==X.Y.Z` suelto, para que la versión de PyInstaller (y sus
    dependencias transitivas) sea tan reproducible como el resto del build.
    """
    with StageTimer("CheckDeps", "Verificando dependencias"):
        ensure_build_dependency(
            "PyInstaller",
            lambda: module_available("PyInstaller"),
            install_cmd=[
                sys.executable, "-m", "pip", "install",
                "-r", str(PYINSTALLER_LOCKFILE), "--require-hashes",
            ],
            required=True,
        )


def install_lockfile_dependencies(lockfile) -> None:
    """Instala las dependencias de runtime desde `lockfile` (--require-hashes).

    Fuente única para los tres scripts de build: antes cada uno
    duplicaba el chequeo de existencia, el mensaje de log y el try/except de
    subprocess.run con el mismo manejo de timeout/CalledProcessError. Cada
    build_*.py resuelve su propio lockfile (universal o CPU-only-linux según
    la plataforma/arquitectura) y delega la instalación aquí. Aborta el build
    (sys.exit(1)) si el lockfile no existe, si pip falla o si excede
    BUILD_SUBPROCESS_TIMEOUT: un build sin dependencias reproducibles nunca
    debe continuar en silencio.
    """
    lockfile = Path(lockfile)
    if not lockfile.exists():
        log(f"ERROR: No se encontró {lockfile}; instala primero con: pip install -r {lockfile.name} --require-hashes")
        sys.exit(1)

    log(f"Instalando dependencias runtime desde {lockfile.name}...")
    try:
        subprocess.run(
            [sys.executable, "-m", "pip", "install", "-r", str(lockfile), "--require-hashes"],
            check=True,
            timeout=BUILD_SUBPROCESS_TIMEOUT,
        )
    except subprocess.CalledProcessError as exc:
        log(f"ERROR: Falló la instalación del lockfile (rc={exc.returncode})")
        sys.exit(1)
    except subprocess.TimeoutExpired:
        log(f"ERROR: La instalación del lockfile excedió {BUILD_SUBPROCESS_TIMEOUT}s")
        sys.exit(1)


def check_sounddevice() -> None:
    """Verifica que sounddevice esté instalado (dependencia del producto).

    Fuente única para build_linux.py y build_macos.py: en Linux
    gobierna reproducción + enumeración de dispositivos; en macOS solo la
    enumeración (afplay reproduce). Sin pin: es dependencia de runtime
    gobernada por requirements.txt, no una herramienta de build. required:
    sin ella el bundle saldría sin audio funcional (doctor/setup/devices
    fallarían en todo binario congelado).
    """
    ensure_build_dependency(
        "sounddevice",
        lambda: module_available("sounddevice"),
        install_cmd=[sys.executable, "-m", "pip", "install", "sounddevice"],
        required=True,
    )


def common_pyinstaller_args(
    entry_point: Path,
    project_root: Path,
    dist_dir: Path,
    build_dir: Path,
    data_sep: str,
    extra_collect_all=(),
    extra_collect_binaries=(),
) -> list:
    """Flags de PyInstaller compartidas por los tres scripts de build.

    `data_sep` es el separador de `--add-data` según el SO: ';' en Windows,
    ':' en Linux/macOS. `extra_collect_all` añade paquetes `--collect-all`
    específicos de la plataforma (p. ej. pycaw en Windows, sounddevice en
    Linux y macOS). `extra_collect_binaries` añade `--collect-binaries` para
    paquetes que requieren bibliotecas compartidas del intérprete (p. ej.
    python en macOS para incluir libpython3.13.dylib).
    """
    return [
        sys.executable, "-m", "PyInstaller",
        "--onedir",
        "--console",
        "--name", "ai-voice-interconnector",
        "--paths", str(project_root / "src"),
        "--distpath", str(dist_dir),
        "--workpath", str(build_dir),
        "--specpath", str(project_root / "scripts"),
        "--noconfirm",
        # Sin compresión UPX: el empaquetado UPX es una de las señales que la
        # heurística de los antivirus asocia con malware (binario que se
        # autodescomprime en memoria); --noupx la elimina en los tres builds
        # a cambio de bundles algo más grandes (docs/SELF-HOSTED-INSTALL.md).
        "--noupx",
        str(entry_point),
        # Recolectar todos los paquetes que PyInstaller no puede seguir
        # automáticamente (imports perezosos, extensiones C, código compilado)
        "--collect-all", "chatterbox",
        "--collect-all", "transformers",
        "--collect-all", "diffusers",
        "--collect-all", "s3tokenizer",
        "--collect-all", "perth",
        "--collect-all", "librosa",
        "--collect-all", "torch",
        "--collect-all", "ctranslate2",
        "--collect-all", "sklearn",
        "--collect-all", "pandas",
        "--collect-all", "onnx",
        "--collect-all", "faster_whisper",
        "--collect-all", "av",
        "--collect-all", "onnxruntime",
        "--collect-all", "miniaudio",
        *[flag for pkg in extra_collect_all for flag in ("--collect-all", pkg)],
        # Bibliotecas compartidas del intérprete (macOS requiere python para libpython3.x.dylib)
        *[flag for pkg in extra_collect_binaries for flag in ("--collect-binaries", pkg)],
        # Data files
        "--collect-data", "soundfile",
        "--collect-data", "certifi",
        # Voces de fábrica (incluida la voz 'default'), empaquetadas dentro del
        # árbol del paquete y resueltas en runtime por paths.bundled_voices_dir()
        # (sys._MEIPASS/ai_voice_interconnector/voices).
        # Fuente ÚNICA de las voces en CI: `--add-data`. A propósito NO se usa
        # `--collect-all ai_voice_interconnector` para datos, porque en el build de CI el
        # proyecto no se instala editable (solo requirements-lock*.txt), así que
        # durante la fase SPEC PyInstaller emite "collect_data_files - skipping
        # data collection for module 'ai_voice_interconnector' as it is not a package" y no
        # aporta nada. `--add-data` es más robusto: falla ruidosamente si falta
        # el dir fuente, en vez de enmascarar la ausencia de voces en silencio.
        "--add-data", f"{project_root / 'src' / 'ai_voice_interconnector' / 'voices'}{data_sep}ai_voice_interconnector/voices",
        # Metadata requerida por importlib.metadata / pkg_resources
        "--recursive-copy-metadata", "chatterbox-tts",
        "--copy-metadata", "requests",
        # Excluir bloat (nunca cargado en runtime)
        "--exclude-module", "tensorflow",
        "--exclude-module", "jax",
        "--exclude-module", "flax",
        "--exclude-module", "gradio",
        "--exclude-module", "gradio_client",
    ]


def bundle_size_mb(onedir) -> float:
    """Calcula el tamaño total en MB de un directorio de bundle (onedir de PyInstaller).

    Fuente única para los tres scripts de build, que antes duplicaban este
    mismo cálculo.
    """
    total_bytes = sum(f.stat().st_size for f in Path(onedir).rglob("*") if f.is_file())
    return total_bytes / (1024 * 1024)


def copy_license_files(dest_dir) -> None:
    """Copia los LICENSE_FILES desde la raíz del proyecto a dest_dir.

    Se invoca tras PyInstaller en cada plataforma para que el bundle distribuible
    (onedir de Windows/Linux, .app de macOS) incluya los avisos de licencia que
    GPLv3 y las licencias permisivas de terceros exigen preservar al redistribuir.
    """
    project_root = Path(__file__).parent.parent
    dest = Path(dest_dir)
    dest.mkdir(parents=True, exist_ok=True)
    for name in LICENSE_FILES:
        src = project_root / name
        if src.exists():
            shutil.copy2(src, dest / name)
            log(f"Licencia empaquetada: {name} -> {dest}")
        else:
            log(f"WARNING: no se encontró {name} en la raíz; no se empaquetó")


def ensure_png_icon(dest_path) -> Path:
    """Copia el logo del proyecto a dest_path como icono PNG.

    Si LOGO_SOURCE no existe, materializa un PNG 1×1 transparente como
    placeholder (preserva el comportamiento previo del AppImage, que exige un
    icono presente en el AppDir). Devuelve siempre la ruta de destino.
    """
    dest = Path(dest_path)
    dest.parent.mkdir(parents=True, exist_ok=True)
    if LOGO_SOURCE.exists():
        shutil.copy2(LOGO_SOURCE, dest)
        log(f"Icono PNG: {LOGO_SOURCE.name} -> {dest}")
    else:
        dest.write_bytes(base64.b64decode(_PLACEHOLDER_PNG_1X1))
        log(f"WARNING: no se encontró {LOGO_SOURCE}; se usó placeholder 1×1 en {dest}")
    return dest


def _generate_pillow_icon(dest_dir, filename: str, save_kwargs: dict, label: str, fallback_desc: str) -> Path:
    """Genera un icono nativo desde LOGO_SOURCE con Pillow.

    Factoriza la lógica antes duplicada entre ensure_ico y ensure_icns:
    apertura del logo, creación del directorio destino, guardado con Pillow y
    logging (éxito o warning de degradación). `save_kwargs` son los kwargs de
    `Image.save` (formato y, en el .ico, los tamaños multi-resolución).
    `label` identifica el formato en mayúsculas (p. ej. "ICO"); el mensaje de
    warning deriva su extensión en minúsculas del mismo label. Devuelve la
    ruta generada, o None si Pillow o la fuente del logo faltan (degradación
    con gracia: el llamador sigue sin icono nativo).
    """
    try:
        from PIL import Image

        dest = Path(dest_dir)
        dest.mkdir(parents=True, exist_ok=True)
        icon_path = dest / filename
        with Image.open(LOGO_SOURCE) as img:
            img.save(icon_path, **save_kwargs)
        log(f"Icono {label} generado: {icon_path}")
        return icon_path
    except (ImportError, FileNotFoundError, OSError) as exc:
        log(f"WARNING: no se pudo generar el .{label.lower()} ({exc}); {fallback_desc}")
        return None


def ensure_ico(dest_dir) -> Path:
    """Genera un .ico multi-resolución (16/32/48/256) desde el logo del proyecto.

    Devuelve la ruta del .ico generado, o None si Pillow o la fuente faltan
    (degradación con gracia: el build sigue sin icono nativo).
    """
    return _generate_pillow_icon(
        dest_dir, "ai-voice-interconnector.ico",
        {"format": "ICO", "sizes": [(16, 16), (32, 32), (48, 48), (256, 256)]},
        "ICO", "build sin icono nativo",
    )


def ensure_icns(dest_dir) -> Path:
    """Genera un .icns desde el logo del proyecto para el bundle .app de macOS.

    Devuelve la ruta del .icns generado, o None si Pillow o la fuente faltan
    (degradación con gracia: el .app mantiene el CFBundleIconFile vacío).
    """
    # Pillow escribe .icns a partir de la imagen fuente; los tamaños del
    # iconset se derivan internamente. El logo es 256×256, tamaño válido.
    return _generate_pillow_icon(
        dest_dir, "ai-voice-interconnector.icns",
        {"format": "ICNS"},
        "ICNS", ".app sin icono nativo",
    )


def get_version(init_path: Path = None) -> str:
    """Lee la versión de src/ai_voice_interconnector/__init__.py.

    Fuente única de versión para los tres scripts de build (Windows, Linux,
    macOS) y el generador del instalador Inno Setup. `init_path` permite
    apuntar a otro __init__.py (tests).
    """
    if init_path is None:
        init_path = Path(__file__).parent.parent / "src" / "ai_voice_interconnector" / "__init__.py"
    for line in init_path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line.startswith("__version__"):
            parts = line.split("=", 1)
            if len(parts) == 2:
                return parts[1].strip().strip('"').strip("'")
    raise RuntimeError("Could not find __version__ in __init__.py")


def _format_duration(seconds: float) -> str:
    """Formatea segundos a string legible.

    - < 60s       → '45.2s'
    - 60s–3599s   → '1m 32.5s'
    - ≥ 3600s     → '1h 23m 45.0s'
    """
    if seconds >= 3600:
        h = int(seconds // 3600)
        remainder = seconds - h * 3600
        m = int(remainder // 60)
        s = remainder - m * 60
        return f"{h}h {m}m {s:.1f}s"
    if seconds >= 60:
        m = int(seconds // 60)
        s = seconds - m * 60
        return f"{m}m {s:.1f}s"
    return f"{seconds:.1f}s"


def log(msg: str, duration: float = None):
    """Imprime un mensaje de log con formato consistente.

    Sin duration: [HH:MM:SS] Mensaje...
    Con duration: [HH:MM:SS] Mensaje -> Done (1m 32s)
    """
    now = datetime.now().strftime("%H:%M:%S")
    if duration is not None:
        print(f"[{now}] {msg} -> Done ({_format_duration(duration)})")
    else:
        print(f"[{now}] {msg}...")


class StageTimer:
    """Context manager para temporizar una etapa de build.

    Uso:
        with StageTimer("NombreEtapa", "Descripción"):
            # código a temporizar
    """

    def __init__(self, name: str, description: str = None):
        self.name = name
        self.description = description or name
        self.start = None

    def __enter__(self):
        self.start = time.time()
        print()
        log(f"[{self.name}] {self.description}")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        duration = time.time() - self.start
        log(f"[{self.name}]", duration)
        print()
        return False


class BuildTimer:
    """Context manager para temporizar el proceso de build completo.

    A diferencia de StageTimer (que mide etapas individuales), BuildTimer
    envuelve todo el build e imprime cabeceras de inicio y fin globales.
    """

    def __init__(self):
        self.start = None
        self.duration = None

    def __enter__(self):
        self.start = time.time()
        print()
        log("=== BUILD INICIADO ===")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        self.duration = time.time() - self.start
        if exc_type is None:
            log("=== BUILD COMPLETADO ===", self.duration)
        else:
            log("=== BUILD FALLIDO ===", self.duration)
        print()
        return False


def kill_process_tree(pid: int) -> None:
    """Mata el proceso `pid` y todos sus descendientes.

    Red de seguridad para el timeout de run_pyinstaller: en el cuelgue COM de
    Windows, matar solo al hijo directo no basta — el proceso de análisis deja
    un zombie que retiene los pipes heredados y mantiene el job de CI colgado
    (evidencia commit 524b18e). Se mata el árbol completo: en Windows con
    `taskkill /T /F`, en POSIX con `os.killpg` sobre el grupo de procesos.
    """
    if sys.platform == "win32":
        subprocess.run(
            ["taskkill", "/T", "/F", "/PID", str(pid)],
            capture_output=True,
        )
    else:
        try:
            os.killpg(os.getpgid(pid), signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass


def run_pyinstaller(pyinstaller_args: list, timeout: int) -> int:
    """Lanza PyInstaller heredando la consola y devuelve su returncode.

    En Linux/macOS ejecuta `pyinstaller_args` directo. En Windows los reescribe
    para lanzarlos a través de scripts/pyinstaller_wrapper.py, que setea
    `sys.coinit_flags = 0x8` antes de importar comtypes y sale con os._exit,
    evitando el cuelgue COM del análisis (ver el docstring del wrapper).
    `pyinstaller_args` empieza con `[sys.executable, "-m", "PyInstaller", ...]`;
    el wrapper reemplaza ese prefijo de 3 elementos, así que se le pasan solo los
    args de PyInstaller (`pyinstaller_args[3:]`) — pasar `[2:]` dejaba un
    "PyInstaller" colgando como primer argumento y rompía la invocación
    (evidencia pipeline #22).

    No se capturan pipes: el output de PyInstaller hace de heartbeat para CI. En
    timeout mata el árbol de procesos (kill_process_tree) y devuelve 1.
    """
    if sys.platform == "win32":
        wrapper = Path(__file__).parent / "pyinstaller_wrapper.py"
        cmd = [sys.executable, str(wrapper), *pyinstaller_args[3:]]
        proc = subprocess.Popen(cmd)
    else:
        # Grupo de procesos propio para poder matar el árbol completo en timeout.
        proc = subprocess.Popen(pyinstaller_args, start_new_session=True)

    try:
        return proc.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        log(f"[TIMEOUT] PyInstaller excedió {timeout}s; matando el árbol de procesos")
        kill_process_tree(proc.pid)
        try:
            proc.wait(timeout=30)
        except subprocess.TimeoutExpired:
            pass
        return 1
