"""
Almacén de habla sintética, libre de modelo.

Hogar único de las locuciones grabadas por el grupo `speech`: cada toma se
persiste como un `.wav` en `data_root()/synthetic-speech/<voz>/<etiqueta>.wav`
con un sidecar `<etiqueta>.json` de metadatos (`text`, `voice`, `created_at`).
Es hermano de `voices.py` —operaciones puras de filesystem, sin modelo— y vive
sobre la misma raíz escribible de `paths.data_root()`.

El **WAV es el recurso de registro**: su presencia decide la existencia, la
colisión y la enumeración. El sidecar es metadato derivado y tolerante a su
ausencia (una toma escrita a medias, o un sidecar borrado a mano, no rompe
`list_entries`). La escritura es atómica: sidecar primero y WAV después, cada
uno a un temporal en el mismo directorio publicado con `os.replace`, de modo que
un `.wav` presente siempre tiene su metadato ya en disco.

El daemon nunca toca este almacén: solo el cliente lo escribe y lo lee.
"""

import json
import os
import tempfile
from datetime import datetime, timezone

from . import paths, voices


def store_root() -> str:
    """Directorio base del almacén de habla sintética (escribible)."""
    return os.path.join(paths.data_root(), "synthetic-speech")


def voice_store_dir(voice: str) -> str:
    """Directorio del namespace de una voz concreta dentro del almacén.

    Aplica la misma defensa en profundidad que `voices.voice_dir`: valida el
    segmento y comprueba con realpath que la ruta resuelta quede dentro del
    almacén, cerrando la clase de escapes de ruta.
    """
    voice = voices._validate_path_segment(voice, kind="voz")  # Normaliza a minúsculas
    root = store_root()
    target = os.path.join(root, voice)
    real_root = os.path.realpath(root)
    if os.path.realpath(target) != os.path.join(real_root, voice):
        raise ValueError(f"Nombre de voz inválido: {voice!r} (escapa del almacén de habla sintética)")
    return target


def _resolve(voice: str, label: str) -> tuple[str, str]:
    """Resuelve (voz, etiqueta) a las rutas (wav, sidecar), validando ambos segmentos."""
    label = voices._validate_path_segment(label, kind="etiqueta")  # Normaliza a minúsculas
    directory = voice_store_dir(voice)
    wav = os.path.join(directory, f"{label}.wav")
    sidecar = os.path.join(directory, f"{label}.json")
    return (wav, sidecar)


def _atomic_write(path: str, data: bytes) -> None:
    """Publica `data` en `path` de forma atómica vía temporal + os.replace.

    El temporal se crea en el mismo directorio que el destino para que
    `os.replace` sea un rename dentro del mismo filesystem (atómico en POSIX y
    Windows). El descriptor se cierra antes del replace: en Windows un archivo
    abierto no puede ser reemplazado.
    """
    directory = os.path.dirname(path)
    fd, tmp = tempfile.mkstemp(dir=directory, suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.replace(tmp, path)
    except BaseException:
        # Si algo falla antes del replace, no dejar el temporal huérfano.
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise


def save(voice: str, label: str, wav_bytes: bytes, text: str) -> str:
    """Persiste una locución: sidecar primero, WAV después, ambos atómicos.

    Devuelve la ruta del `.wav`. El orden (sidecar → WAV) garantiza que un WAV
    presente en disco siempre tiene su metadato disponible; la operación inversa
    dejaría una toma reproducible sin su `text`/`created_at`.
    """
    wav, sidecar = _resolve(voice, label)
    os.makedirs(os.path.dirname(wav), exist_ok=True)
    metadata = {
        "voice": voices._validate_path_segment(voice, kind="voz"),
        "label": voices._validate_path_segment(label, kind="etiqueta"),
        "text": text,
        "created_at": datetime.now(timezone.utc).isoformat(),
    }
    _atomic_write(sidecar, json.dumps(metadata, ensure_ascii=False, indent=2).encode("utf-8"))
    _atomic_write(wav, wav_bytes)
    return wav


def exists(voice: str, label: str) -> bool:
    """True si la locución existe (la decide el WAV, no el sidecar)."""
    wav, _ = _resolve(voice, label)
    return os.path.exists(wav)


def wav_path(voice: str, label: str) -> str:
    """Ruta del WAV de una locución existente.

    Raises:
        FileNotFoundError: si la locución no existe.
    """
    wav, _ = _resolve(voice, label)
    if not os.path.exists(wav):
        raise FileNotFoundError(
            f"Locución '{label}' de la voz '{voice}' no encontrada en el almacén de habla sintética."
        )
    return wav


def remove(voice: str, label: str) -> bool:
    """Borra el WAV y el sidecar de una locución. Devuelve True si algo existía.

    Tolera la ausencia de cualquiera de los dos archivos (un sidecar huérfano o
    un WAV sin metadato se limpian igual).
    """
    wav, sidecar = _resolve(voice, label)
    existed = False
    for path in (wav, sidecar):
        if os.path.exists(path):
            os.unlink(path)
            existed = True
    return existed


def _read_sidecar(sidecar: str) -> dict:
    """Lee el sidecar de una locución, tolerando su ausencia o corrupción."""
    try:
        with open(sidecar, "r", encoding="utf-8") as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def list_entries(voice: str | None = None) -> list[dict]:
    """Enumera las locuciones del almacén, opcionalmente filtradas por voz.

    Enumera **por los `.wav`** (el recurso de registro) y adjunta los metadatos
    del sidecar si están, rellenando `text`/`created_at` con `None` cuando el
    sidecar falta o está corrupto. Cada entrada es un dict
    `{"voice", "label", "text", "created_at"}`. El orden es determinista:
    alfabético por voz y, dentro de cada voz, por etiqueta.
    """
    root = store_root()
    if not os.path.exists(root):
        return []

    if voice is not None:
        voice = voices._validate_path_segment(voice, kind="voz")
        voice_dirs = [voice] if os.path.isdir(os.path.join(root, voice)) else []
    else:
        voice_dirs = sorted(
            entry for entry in os.listdir(root) if os.path.isdir(os.path.join(root, entry))
        )

    entries: list[dict] = []
    for voice_name in voice_dirs:
        directory = os.path.join(root, voice_name)
        wavs = sorted(name for name in os.listdir(directory) if name.endswith(".wav"))
        for wav_name in wavs:
            label = wav_name[: -len(".wav")]
            metadata = _read_sidecar(os.path.join(directory, f"{label}.json"))
            entries.append(
                {
                    "voice": voice_name,
                    "label": label,
                    "text": metadata.get("text"),
                    "created_at": metadata.get("created_at"),
                }
            )
    return entries
