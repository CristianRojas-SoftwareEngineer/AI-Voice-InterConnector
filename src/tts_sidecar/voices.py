"""
Gestión del registro de voces, libre de modelo.

Hogar único de las rutas del directorio de voces y de las operaciones de registro
(listar, eliminar, resolver rutas). El modelo de voces es de **dos niveles**,
uniforme en los tres modos de ejecución (fuente, pip/uv-installed, congelado):

- **Usuario**: voces escribibles en `data_root()/voices` (user-data-dir por SO).
- **Fábrica**: voces de solo lectura empaquetadas en `bundled_voices_dir()`
  (subdirectorio `voices` del paquete `tts_sidecar`; `sys._MEIPASS` congelado).

La resolución de un nombre busca primero en usuario y luego en fábrica, de modo
que un usuario puede sobrescribir una voz de fábrica registrando una propia con
el mismo nombre. Ninguna función aquí importa ni carga el modelo: son operaciones
puras de sistema de archivos.
"""

import os
import re
import shutil

from . import paths

# Nombres de voz admitidos: un único segmento de ruta sin separadores ni escapes.
_VOICE_NAME_RE = re.compile(r"^[A-Za-z0-9._-]+$")


def _validate_path_segment(value: str, kind: str = "voz") -> str:
    """Valida un segmento de ruta (p. ej. nombre de voz) antes de componer cualquier ruta con él.

    Rechaza valores vacíos, con separadores de ruta, rutas absolutas o `..`,
    eliminando la clase de escapes de ruta (p. ej. `voice remove --name ..`
    resolvería al padre del registro y lo borraría).

    Normaliza a minúsculas para evitar colisiones en filesystems
    case-insensitive (macOS APFS, Docker volumes sobre NTFS, etc.). La defensa
    anti-escape usa realpath así que el directorio real siempre queda dentro del registro.
    """
    if not value or not _VOICE_NAME_RE.match(value) or ".." in value or value == ".":
        raise ValueError(
            f"Nombre de {kind} inválido: {value!r}. "
            "Usa solo letras, números, punto, guion y guion bajo (sin '..' ni separadores de ruta)."
        )
    return value.lower()


def voices_root() -> str:
    """Directorio base de las voces de usuario (escribible)."""
    return os.path.join(paths.data_root(), "voices")


def factory_voices_root() -> str:
    """Directorio base de las voces de fábrica (solo lectura)."""
    return paths.bundled_voices_dir()


def voice_dir(name: str) -> str:
    """Directorio de una voz de usuario concreta (destino de escritura)."""
    name = _validate_path_segment(name, kind="voz")  # Normaliza a minúsculas
    root = voices_root()
    target = os.path.join(root, name)
    # Defensa en profundidad: la ruta resuelta debe quedar dentro del registro.
    real_root = os.path.realpath(root)
    if os.path.realpath(target) != os.path.join(real_root, name):
        raise ValueError(f"Nombre de voz inválido: {name!r} (escapa del registro de voces)")
    return target


def _is_symlink(path: str) -> bool:
    """True si `path` es un symlink, sin seguirlo.

    `os.path.islink` inspecciona solo el segmento nombrado (no sus ancestros),
    de modo que un enlace en `registry/<nombre>` o en `timbre-reference.wav` se detecta
    sin recorrer el árbol ni rechazar raíces legítimas que el usuario haya
    enlazado (p. ej. `data_root` mismo). Es portable a Windows, a
    diferencia de `O_NOFOLLOW` (POSIX).
    """
    return os.path.islink(path)


def _is_valid_voice_dir(candidate: str) -> bool:
    """Una voz es válida solo con sus dos audios: timbre-reference.wav (timbre) y
    speech-reference.wav (conditioning), igual que exige `voice clone`.

    Cualquier componente symlink (el directorio de la voz o sus dos
    `.wav`) la hace inválida, para que `list_voices` y `voice_paths` coincidan
    en rechazarla — un symlink dentro del registro no puede cargar un `.wav`
    arbitrario del atacante. La defensa en profundidad de escape ya vive en
    `voice_dir` (realpath); aquí se cierra la ventana de symlink a la raíz.
    """
    if _is_symlink(candidate):
        return False
    ref = os.path.join(candidate, "timbre-reference.wav")
    speech = os.path.join(candidate, "speech-reference.wav")
    if _is_symlink(ref) or _is_symlink(speech):
        return False
    return os.path.exists(ref) and os.path.exists(speech)


def _resolve_voice_dir(name: str) -> str | None:
    """Devuelve el directorio de una voz con precedencia usuario→fábrica, o None."""
    _validate_path_segment(name, kind="voz")
    for root in (voices_root(), factory_voices_root()):
        candidate = os.path.join(root, name)
        if _is_valid_voice_dir(candidate):
            return candidate
    return None


def list_voices() -> list[str]:
    """Listar todas las voces disponibles (usuario + fábrica, sin duplicados).

    `seen` es un `set` (membership check O(1)) que solo decide la dedupe; el
    orden de salida lo sigue dando `result`, poblado en el mismo orden de
    iteración que antes (usuario→fábrica, alfabético dentro de cada raíz).
    """
    seen: set[str] = set()
    result: list[str] = []
    for root in (voices_root(), factory_voices_root()):
        if not os.path.exists(root):
            continue
        for entry in sorted(os.listdir(root)):
            candidate = os.path.join(root, entry)
            if entry not in seen and os.path.isdir(candidate) and _is_valid_voice_dir(candidate):
                seen.add(entry)
                result.append(entry)
    return result


def remove_voice(name: str) -> bool:
    """Eliminar una voz de usuario. Devuelve True si existía y se borró.

    Solo opera sobre voces de usuario; las de fábrica son de solo lectura.
    """
    target = voice_dir(name)
    # Solo se borran directorios que sean voces válidas: un directorio arbitrario
    # dentro del registro (o el registro mismo) nunca es objetivo de rmtree.
    if os.path.exists(target) and _is_valid_voice_dir(target):
        shutil.rmtree(target)
        return True
    return False


def clone_voice_files(
    name: str,
    timbre_reference: str,
    speech_reference: str,
    force: bool = False,
) -> tuple[str, str]:
    """Valida y copia los audios de una voz como clon de usuario, SIN el modelo.

    Es el núcleo de `voice clone`: validación de carga con librosa (audio ilegible
    no debe dejar una voz rota que falle recién en la síntesis), colisión de
    nombres con precedencia usuario→fábrica, y copia de los dos WAV. No importa
    torch ni instancia el motor: la precomputación de conditionals se difiere al
    primer `speak` con la voz.

    Raises:
        ValueError: si algún audio no es cargable, o si la voz ya existe
                    (usuario o fábrica) y no se pasó force.
    """
    # Import local: librosa es pesada y solo la necesita este comando.
    import librosa
    for label, path in (("reference", timbre_reference), ("speech", speech_reference)):
        try:
            librosa.load(path, sr=24000, duration=1.0)
        except Exception as e:
            raise ValueError(f"El audio de {label} ({path}) no es cargable: {e}")

    # La colisión con una voz existente (usuario o fábrica homónima) exige --force
    if not force and _resolve_voice_dir(name) is not None:
        raise ValueError(f"La voz '{name}' ya existe. Usa --force para sobrescribirla.")

    target = voice_dir(name)
    os.makedirs(target, exist_ok=True)
    # Ni el directorio destino ni los dos WAV pueden ser symlinks;
    # si lo fueran, copy2 escribiría *a través* del enlace (y voice_paths
    # ya los rechaza al leer). Se rechaza antes de tocar el filesystem.
    if _is_symlink(target):
        raise ValueError(f"La voz '{name}' apunta a un symlink; no se puede clonar.")
    ref_path = os.path.join(target, "timbre-reference.wav")
    speech_path = os.path.join(target, "speech-reference.wav")
    shutil.copy2(timbre_reference, ref_path)
    shutil.copy2(speech_reference, speech_path)
    return (ref_path, speech_path)


def voice_paths(name: str) -> tuple[str, str]:
    """
    Resolver el nombre de una voz a sus rutas de audio (reference, speech).

    Busca con precedencia usuario→fábrica y valida la existencia de ambos archivos.
    """
    target = _resolve_voice_dir(name)
    if target is None:
        raise FileNotFoundError(
            f"Voz '{name}' no encontrada (ni en las voces de usuario ni en las de fábrica). "
            f"Clónala con 'tts-sidecar voice clone' o usa la voz 'default'."
        )
    ref_path = os.path.join(target, "timbre-reference.wav")
    speech_path = os.path.join(target, "speech-reference.wav")
    if not os.path.exists(ref_path):
        raise FileNotFoundError(f"Voz '{name}': timbre-reference.wav no encontrado en {ref_path}")
    if not os.path.exists(speech_path):
        raise FileNotFoundError(f"Voz '{name}': speech-reference.wav no encontrado en {speech_path}")
    return (ref_path, speech_path)
