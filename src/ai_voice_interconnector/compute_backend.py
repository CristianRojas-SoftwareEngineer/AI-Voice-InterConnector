"""
Resolución del compute backend (cuda/mps/cpu) y clave de caché de instancias.

Extraído de `ChatterboxEngine` para desacoplar la lógica de selección
de backend y la construcción de la clave de caché compartida con el daemon
(`daemon/run.py`), dejando al engine como façade de síntesis.
"""

import logging

import torch

logger = logging.getLogger(__name__)


class ComputeBackendResolver:
    """Resuelve el backend de cómputo disponible y la clave de caché de instancias."""

    @staticmethod
    def detect() -> str:
        """Resuelve el mejor compute backend disponible en el host.

        Orden de preferencia: cuda (NVIDIA) → mps (Apple Silicon) → cpu.
        Los probes de torch se envuelven en try/except: un torch sin CUDA
        o sin MPS, o un fallo de import del backend, degradan a "cpu" sin
        crashear.
        """
        try:
            if torch.cuda.is_available():
                return "cuda"
        except Exception:
            logger.debug("La prueba de disponibilidad de CUDA falló; se descarta el backend", exc_info=True)
        try:
            if hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
                return "mps"
        except Exception:
            logger.debug("La prueba de disponibilidad de MPS falló; se descarta el backend", exc_info=True)
        return "cpu"

    @staticmethod
    def resolve(compute_backend: str | None) -> str:
        """Acepta None/"auto" y lo mapea a uno de los backends concretos."""
        if compute_backend is None or compute_backend == "auto":
            return ComputeBackendResolver.detect()
        return compute_backend

    @staticmethod
    def cache_key(model: str = "es-mx-latam", compute_backend: str = "cpu", models_dir=None) -> str:
        """Construye la clave de caché de instancias, compartida con el daemon (run.py)."""
        return f"{model}:{compute_backend}:{models_dir}"
