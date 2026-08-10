"""
Detección de modelos en la caché de HuggingFace, libre de dependencias de ML.

Este módulo NO importa torch ni chatterbox: los comandos que solo necesitan
saber si el modelo está descargado (síntesis vía daemon, doctor, setup) lo usan
sin pagar los segundos de arranque en frío de engine.py.
"""

import os
from pathlib import Path
from typing import Optional

# Alias de modelo expuestos por el CLI → repo de HuggingFace
MODELS = {
    "es-mx-latam": "ResembleAI/Chatterbox-Multilingual-es-mx-latam",
    # Modelo inglés base, promovido a entrada de primer nivel (rediseño
    # cross-lingual): mismo repo que BASE_MODEL_REPO, fuente también de
    # ve.safetensors para es-mx-latam.
    "en": "ResembleAI/chatterbox",
}

# Repo base de Chatterbox: fuente de ve.safetensors (Voice Encoder), que el
# language pack es-mx-latam no incluye.
BASE_MODEL_REPO = "ResembleAI/chatterbox"

# Revisiones fijadas de los repos del modelo: commit hash de HuggingFace
# auditado por release. 'setup' descarga exactamente estas revisiones y la
# detección de caché valida contra su snapshot, de modo que un push posterior
# (malicioso o accidental) al repo del modelo no se propaga a los usuarios.
#
# Proceso de actualización (parte del runbook de release, ver docs/RELEASING.md):
#   1. Consultar el sha vigente:  https://huggingface.co/api/models/<repo>  (campo "sha")
#   2. Auditar el diff de la revisión nueva en la pestaña "Files" del repo de HF.
#   3. Reemplazar el hash aquí y verificar con 'setup --force-update' + 'doctor'.
MODEL_REVISIONS = {
    "es-mx-latam": "27e595bf2fe7be0533ca299d9afafcde08b7cca7",
}
BASE_MODEL_REVISION = "5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18"
# El inglés base pinea la misma revisión que BASE_MODEL_REVISION: es el mismo
# repo (ResembleAI/chatterbox), así que comparten el mismo commit auditado.
MODEL_REVISIONS["en"] = BASE_MODEL_REVISION

# Patrones de archivos descargados por snapshot_download por modelo.
# Permite filtrar solo los checkpoints de inferencia, evitando descargar
# variantes no usadas (p. ej. las 5 variantes T3 del repo base que pesan ~8 GB).
MODEL_ALLOW_PATTERNS = {
    "es-mx-latam": [
        "t3_es_mx_latam.safetensors",
        "s3gen_v3.safetensors",
        "s3gen_v3.pt",
        "grapheme_mtl_merged_expanded_v1.json",
    ],
    "en": [
        "t3_cfg.safetensors",
        "s3gen.safetensors",
        "ve.safetensors",
        "ve.pt",
        "tokenizer.json",
        "conds.pt",
    ],
}

# Mapa único de alias/repos al nombre de carpeta en la caché de HuggingFace
CACHE_NAMES = {
    "es-mx-latam": "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam",
    "ResembleAI/Chatterbox-Multilingual-es-mx-latam": "models--ResembleAI--Chatterbox-Multilingual-es-mx-latam",
}


def hub_cache_path() -> Path:
    """Raíz de la caché de HuggingFace usada por el proyecto.

    Delega en huggingface_hub.constants.HF_HUB_CACHE: la misma fuente que
    consulta snapshot_download, de modo que respeta HF_HUB_CACHE/HF_HOME y
    no puede divergir de donde 'setup' descarga realmente el modelo.
    """
    # Import local: huggingface_hub es liviano pero se difiere igualmente
    # para no encarecer los comandos que nunca tocan la caché.
    from huggingface_hub import constants
    return Path(constants.HF_HUB_CACHE)


def cache_folder_for(model_name: str) -> str:
    """Nombre de la carpeta de caché para un alias o repo de modelo."""
    return CACHE_NAMES.get(model_name, f"models--{model_name.replace('/', '--')}")


def model_for(language: str) -> str:
    """Traduce un idioma de la taxonomía compartida (es-latam/en/all) al alias
    de modelo que consumen MODELS/MODEL_REVISIONS/is_model_cached.

    "all" no es un idioma de síntesis válido (solo de provisión/precarga); se
    devuelve tal cual para que el llamador lo maneje explícitamente.
    """
    return {"es-latam": "es-mx-latam", "en": "en"}.get(language, language)


def _safetensors_header_ok(path: Path) -> bool:
    """Validación ligera del header de un .safetensors: tamaño del header plausible.

    El formato safetensors empieza con un u64 little-endian que codifica la longitud
    en bytes del header JSON (RFC del formato). Un truncamiento, una descarga a
    medias o un FS corrupto suele dejar un archivo cuyo header-length es 0 o excede
    el tamaño del propio archivo: ambos casos se detectan aquí sin parsear JSON.

    Devuelve True si el header-length está en el rango (0, tamaño_del_archivo). No
    es una validación criptográfica (no hay SHA) pero cubre el escenario de
    caché truncada que pasa el chequeo de existencia y revienta al cargar.
    """
    try:
        size = path.stat().st_size
    except OSError:
        return False
    if size < 8:
        return False
    try:
        with open(path, "rb") as f:
            raw = f.read(8)
    except OSError:
        return False
    header_len = int.from_bytes(raw, byteorder="little", signed=False)
    return 0 < header_len < size


def _resolve_cached_snapshot(
    model_cache_dir: Path, revision: Optional[str] = None
) -> Optional[Path]:
    """
    Resuelve el snapshot vigente de un modelo en la caché de HuggingFace.

    Con una revisión fijada resuelve exclusivamente snapshots/<revision>:
    un snapshot de cualquier otra revisión no cuenta como caché válida. Sin
    revisión, prefiere la apuntada por refs/main (la que huggingface_hub
    considera actual); si el ref no existe o apunta a un snapshot ausente, cae
    al snapshot más reciente por mtime. Devuelve None si no hay ninguno.
    """
    snap_path = model_cache_dir / "snapshots"
    if not snap_path.exists():
        return None

    if revision is not None:
        candidate = snap_path / revision
        return candidate if candidate.is_dir() else None

    ref_main = model_cache_dir / "refs" / "main"
    if ref_main.exists():
        revision = ref_main.read_text(encoding="utf-8").strip()
        candidate = snap_path / revision
        if candidate.is_dir():
            return candidate

    snapshots = [d for d in snap_path.iterdir() if d.is_dir()]
    if not snapshots:
        return None
    return max(snapshots, key=lambda d: d.stat().st_mtime)


def is_ve_cached(lang_snapshot: Optional[Path] = None) -> bool:
    """
    Verifica si ve.safetensors (Voice Encoder) está resoluble localmente.

    El engine lo busca primero en el snapshot del language pack y, si no está,
    en el snapshot cacheado del modelo base (BASE_MODEL_REPO): este chequeo
    replica exactamente esa resolución sin disparar descargas.
    """
    # existencia + validación de header, igual que los otros checkpoints;
    # un ve.safetensors truncado por una descarga a medias pasaba el .exists()
    # y reventaba con un error críptico en la primera síntesis.
    if lang_snapshot is not None:
        ve = lang_snapshot / "ve.safetensors"
        if ve.exists() and _safetensors_header_ok(ve):
            return True
    # La resolución del repo base honra la revisión fijada:
    # un ve.safetensors de otra revisión no cuenta como
    # caché válida, igual que 'setup' lo descarga con revision=BASE_MODEL_REVISION.
    base = _resolve_cached_snapshot(
        hub_cache_path() / cache_folder_for(BASE_MODEL_REPO),
        revision=BASE_MODEL_REVISION,
    )
    if base is None:
        return False
    ve = base / "ve.safetensors"
    return ve.exists() and _safetensors_header_ok(ve)


def is_model_cached(model: str = "es-mx-latam") -> bool:
    """
    Verifica si el modelo dado ya está en la caché de HuggingFace.

    Pura inspección de sistema de archivos: no dispara descargas ni carga
    el modelo en memoria.
    """
    model_name = MODELS.get(model, model)

    # Revisión fijada del modelo: acepta tanto el alias como el repo id.
    revision = MODEL_REVISIONS.get(model)
    if revision is None:
        for alias, repo in MODELS.items():
            if repo == model_name:
                revision = MODEL_REVISIONS.get(alias)
                break

    cached = _resolve_cached_snapshot(
        hub_cache_path() / cache_folder_for(model_name), revision=revision
    )

    if cached is None:
        return False

    # es-mx-latam exige el checkpoint del language-pack más el Voice Encoder
    # (ve.safetensors, compartido con el modelo base): sin él, la primera síntesis
    # dispararía una descarga, rompiendo la promesa «100 % offline tras setup».
    # Además se valida la integridad del header safetensors: un .pt/.st
    # truncado por una descarga a medias pasa el .exists() pero revienta al
    # cargar, así que se reporta como no cacheado para que 'doctor' lo marque
    # FAIL y remita a 'setup' (re-descarga limpia).
    if model == "es-mx-latam" or "es-mx-latam" in model_name:
        # El engine carga tres checkpoints (T3, S3Gen y Voice Encoder);
        # los tres se validan con el mismo chequeo ligero de header, de modo
        # que un truncamiento en cualquiera se reporte como «no cacheado».
        for filename in ("t3_es_mx_latam.safetensors", "s3gen_v3.safetensors"):
            path = cached / filename
            if not path.exists() or not _safetensors_header_ok(path):
                return False
        return is_ve_cached(cached)

    if model == "en" or model_name == BASE_MODEL_REPO:
        # Espejo de la rama es-mx-latam: T3/S3Gen validados con header
        # safetensors; tokenizer.json y conds.pt no son safetensors, solo
        # existencia. El VE es el mismo repo, así que se delega en
        # is_ve_cached(cached) en vez de re-validarlo aparte (evita doble
        # descarga/validación).
        for filename in ("t3_cfg.safetensors", "s3gen.safetensors"):
            path = cached / filename
            if not path.exists() or not _safetensors_header_ok(path):
                return False
        for filename in ("tokenizer.json", "conds.pt"):
            if not (cached / filename).exists():
                return False
        return is_ve_cached(cached)

    return True


def purge_incomplete_downloads() -> int:
    """Borra los blobs '*.incomplete' de las cachés del proyecto; devuelve bytes liberados.

    huggingface_hub deja '<hash>.incomplete' en blobs/ cuando una descarga se
    interrumpe, y solo los reutiliza para reanudar esa misma descarga. Tras una
    provisión exitosa todos los archivos necesarios están completos, así que
    cualquier .incomplete restante es basura huérfana que nadie limpia.
    Se purga únicamente dentro de las carpetas del proyecto (model_cache_dirs),
    nunca la caché completa de HuggingFace; un unlink fallido (archivo en uso,
    permisos) se ignora para no bloquear la provisión.
    """
    freed = 0
    for cache_dir in model_cache_dirs():
        blobs = cache_dir / "blobs"
        if not blobs.is_dir():
            continue
        for leftover in blobs.glob("*.incomplete"):
            try:
                size = leftover.stat().st_size
                leftover.unlink()
                freed += size
            except OSError:
                continue
    return freed


def model_cache_dirs() -> list[Path]:
    """Carpetas de caché de HuggingFace propias del proyecto (existan o no).

    Solo las de los dos repos que el proyecto usa: el language pack es-mx-latam
    y el modelo base (fuente de ve.safetensors). `cleanup` borra exactamente
    estas rutas, nunca la caché completa.
    """
    root = hub_cache_path()
    return [
        root / cache_folder_for("es-mx-latam"),
        root / cache_folder_for(BASE_MODEL_REPO),
    ]


def model_lock_dirs() -> list[Path]:
    """Carpetas de lock (existan o no) que huggingface_hub deja en
    `hub/.locks/<repo>/` junto a cada carpeta de `model_cache_dirs()` durante
    la descarga y no limpia solas tras completarla. Mismo alcance quirúrgico:
    únicamente las de los dos repos del proyecto, nunca `.locks` completo.
    """
    root = hub_cache_path()
    return [root / ".locks" / d.name for d in model_cache_dirs()]
