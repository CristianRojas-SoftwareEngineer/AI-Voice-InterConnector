"""
Carga y resolución de modelos de Chatterbox.

Colaborador extraído de `engine.py`
para aislar la lógica de carga del modelo del engine y hacerla testeable por
inyección, sin instanciar `ChatterboxEngine` ni descargar pesos.

El routing replica exactamente `engine.py:440-443`: la ruta de caché que
contiene "es-mx-latam" usa el loader del language pack; cualquier otra ruta usa
el loader del inglés base (`ChatterboxTTS.from_local`), que desde el rediseño
cross-lingual es una ruta de producción activa (modelo `en`), no un path muerto.
"""

import os
from pathlib import Path

import torch

from safetensors.torch import load_file

from chatterbox.tts import (
    ChatterboxTTS,
    T3,
    S3Gen,
    VoiceEncoder,
)
from chatterbox.mtl_tts import (
    ChatterboxMultilingualTTS,
    MTLTokenizer,
    T3Config,
)

from .model_cache import (
    BASE_MODEL_REVISION,
    _resolve_cached_snapshot,
    cache_folder_for,
    hub_cache_path,
)
from .timing import log


class ModelLoader:
    """Resuelve y carga el modelo Chatterbox según la ruta de caché.

    Inyectable en `ChatterboxEngine`: en producción se instancia por defecto,
    y en tests se puede sustituir por un doble que no toque la caché ni descargue.
    """

    def load(self, cache_dir: Path, model_name: str, compute_backend: str):
        """Carga el modelo según la ruta de caché: es-mx-latam o el multilingüe base."""
        cache_dir = Path(cache_dir)

        # Selección del loader idéntica a engine.py:440-443 (ruta de caché, no alias).
        if "es-mx-latam" in str(cache_dir):
            return self._load_es_latam(cache_dir, compute_backend)
        else:
            return self._load_english_base(cache_dir, compute_backend)

    def _load_es_latam(self, cache_dir: Path, compute_backend: str):
        """
        Carga el language pack es-mx-latam.

        Arquitectura: ChatterboxMultilingualTTS (vocab de 2454 tokens)
        - T3: t3_es_mx_latam.safetensors (entrenado con vocab expandido)
        - S3Gen: s3gen_v3.safetensors
        - Tokenizer: grapheme_mtl_merged_expanded_v1.json (2454 tokens)
        """
        device = compute_backend
        if device in ["cpu", "mps"]:
            map_location = torch.device("cpu")
        else:
            map_location = None

        # Carga el Voice Encoder
        # es-mx-latam no incluye ve.safetensors; se comparte con el modelo base
        ve_path = cache_dir / "ve.safetensors"
        if not ve_path.exists():
            # Intenta la caché del modelo base honrando la revisión fijada
            # (BASE_MODEL_REVISION): la carga resuelve exclusivamente el snapshot
            # del pin, igual que 'setup' lo descarga y la detección lo valida;
            # un bump de revisión no reintroduce el ve.safetensors viejo.
            base_snapshot = _resolve_cached_snapshot(
                hub_cache_path() / cache_folder_for("ResembleAI/chatterbox"),
                revision=BASE_MODEL_REVISION,
            )
            if base_snapshot is not None:
                ve_path = base_snapshot / "ve.safetensors"
        if not ve_path.exists():
            # Red de seguridad: 'setup' provisiona ve.safetensors explícitamente,
            # así que llegar aquí indica una caché podada tras la provisión.
            log(
                "[Codificador de voz] ve.safetensors no está en la caché local; descargándolo ahora. "
                "Ejecuta 'ai-voice-interconnector setup' para reprovisionar la caché completa"
            )
            from huggingface_hub import hf_hub_download
            ve_path = Path(hf_hub_download(
                repo_id="ResembleAI/chatterbox",
                filename="ve.safetensors",
                revision=BASE_MODEL_REVISION,
                token=os.getenv("HF_TOKEN"),
            ))

        ve = VoiceEncoder()
        ve.load_state_dict(load_file(ve_path))
        ve.to(device).eval()

        # Carga el T3 con la config multilingüe (vocab de 2454 tokens)
        t3 = T3(T3Config.multilingual())
        t3_path = cache_dir / "t3_es_mx_latam.safetensors"
        t3_state = load_file(t3_path)
        if "model" in t3_state.keys():
            t3_state = t3_state["model"][0]
        t3.load_state_dict(t3_state)
        t3.to(device).eval()

        # Carga S3Gen v3 (solo safetensors: el .pt duplicado no se descarga;
        # safetensors incluye verificación de integridad en la carga).
        s3gen = S3Gen()
        s3gen_path = cache_dir / "s3gen_v3.safetensors"
        s3gen_state = load_file(s3gen_path)
        s3gen.load_state_dict(s3gen_state, strict=False)
        s3gen.to(device).eval()

        # Carga el tokenizer multilingüe (2454 tokens)
        tokenizer = MTLTokenizer(str(cache_dir / "grapheme_mtl_merged_expanded_v1.json"))

        # Carga los conditionals (voz incorporada del modelo)
        # Esto permite generar sin audio_prompt_path
        conds = None
        conds_path = cache_dir / "conds.pt"
        if conds_path.exists():
            from chatterbox.mtl_tts import Conditionals
            conds = Conditionals.load(conds_path, map_location=map_location).to(device)

        tts = ChatterboxMultilingualTTS(t3, s3gen, ve, tokenizer, device, conds=conds)

        # Si no hay conds incorporados, exige audio_prompt_path en generate()
        tts._require_voice_prompt = conds is None

        log(f"Modelo cargado: es-MX-Latam (vocab=2454, compute_backend={compute_backend}, builtin_voice={'sí' if conds else 'no'})")
        return tts

    def _load_english_base(self, cache_dir: Path, compute_backend: str):
        """Carga el inglés base (path de producción del modelo 'en'): la rama
        else del routing, cualquier ruta de caché que no sea es-mx-latam."""
        return ChatterboxTTS.from_local(cache_dir, compute_backend)
