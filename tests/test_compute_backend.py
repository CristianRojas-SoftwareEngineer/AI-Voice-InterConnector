"""Tests para ComputeBackendResolver: detección, resolución y clave de caché.

El módulo es puro (no carga el modelo); los tests controlan torch vía monkeypatch
para ser deterministas en cualquier host.
"""

import sys
from pathlib import Path
from unittest.mock import patch

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from ai_voice_interconnector.compute_backend import ComputeBackendResolver


class TestDetect:
    def test_prefers_cuda_over_mps_and_cpu(self):
        with patch("torch.cuda.is_available", return_value=True), \
             patch("torch.backends.mps.is_available", return_value=True):
            assert ComputeBackendResolver.detect() == "cuda"

    def test_falls_back_to_mps_when_cuda_unavailable(self):
        with patch("torch.cuda.is_available", return_value=False), \
             patch("torch.backends.mps.is_available", return_value=True):
            assert ComputeBackendResolver.detect() == "mps"

    def test_falls_back_to_cpu_when_no_accelerator(self):
        with patch("torch.cuda.is_available", return_value=False), \
             patch("torch.backends.mps.is_available", return_value=False):
            assert ComputeBackendResolver.detect() == "cpu"

    def test_cuda_probe_failure_degrades_to_cpu(self):
        """Un torch sin CUDA disponible no debe crashear: degrada a cpu."""
        with patch("torch.cuda.is_available", side_effect=RuntimeError("sin CUDA")), \
             patch("torch.backends.mps.is_available", return_value=False):
            assert ComputeBackendResolver.detect() == "cpu"

    def test_mps_probe_failure_degrades_to_cpu(self):
        with patch("torch.cuda.is_available", return_value=False), \
             patch("torch.backends.mps.is_available", side_effect=AttributeError("sin mps")):
            assert ComputeBackendResolver.detect() == "cpu"


class TestResolve:
    def test_none_maps_to_detect(self, monkeypatch):
        monkeypatch.setattr(ComputeBackendResolver, "detect", staticmethod(lambda: "mps"))
        assert ComputeBackendResolver.resolve(None) == "mps"

    def test_auto_maps_to_detect(self, monkeypatch):
        monkeypatch.setattr(ComputeBackendResolver, "detect", staticmethod(lambda: "cuda"))
        assert ComputeBackendResolver.resolve("auto") == "cuda"

    def test_concrete_backend_is_passthrough(self):
        assert ComputeBackendResolver.resolve("cpu") == "cpu"
        assert ComputeBackendResolver.resolve("cuda") == "cuda"
        assert ComputeBackendResolver.resolve("mps") == "mps"


class TestCacheKey:
    def test_default_key(self):
        assert ComputeBackendResolver.cache_key() == "es-mx-latam:cpu:None"

    def test_explicit_fields(self):
        assert ComputeBackendResolver.cache_key("es-mx-latam", "cuda", "/x") == "es-mx-latam:cuda:/x"

    def test_models_dir_distinguishes_key(self):
        a = ComputeBackendResolver.cache_key("es-mx-latam", "cpu", "/ruta/a")
        b = ComputeBackendResolver.cache_key("es-mx-latam", "cpu", "/ruta/b")
        assert a != b
        assert ComputeBackendResolver.cache_key("es-mx-latam", "cpu", "/ruta/a") == a
