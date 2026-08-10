"""Tests de purge_incomplete_downloads.

model_cache.py no tenía cobertura para la limpieza de blobs '*.incomplete':
este archivo crea una caché de HuggingFace sintética en tmp_path (mismo patrón
de monkeypatch de huggingface_hub.constants.HF_HUB_CACHE usado en
test_engine_cache.py) y verifica qué se borra, qué se preserva y qué se cuenta
como bytes liberados.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "src"))

from tts_sidecar.model_cache import (
    BASE_MODEL_REPO,
    MODELS,
    MODEL_ALLOW_PATTERNS,
    cache_folder_for,
    model_cache_dirs,
    purge_incomplete_downloads,
)

ES_MX_FOLDER = cache_folder_for("es-mx-latam")
BASE_FOLDER = cache_folder_for(BASE_MODEL_REPO)


@pytest.fixture
def fake_hub(tmp_path, monkeypatch):
    """Apunta hub_cache_path() a una raíz sintética bajo tmp_path."""
    hub = tmp_path / "hub"
    hub.mkdir()
    from huggingface_hub import constants

    monkeypatch.setattr(constants, "HF_HUB_CACHE", str(hub))
    return hub


def _write_blob(cache_dir: Path, name: str, content: bytes) -> Path:
    blobs = cache_dir / "blobs"
    blobs.mkdir(parents=True, exist_ok=True)
    path = blobs / name
    path.write_bytes(content)
    return path


class TestPurgeIncompleteDownloads:
    def test_no_cache_dirs_returns_zero(self, fake_hub):
        """Ninguna de las dos carpetas del proyecto existe todavía: 0 bytes liberados."""
        assert purge_incomplete_downloads() == 0

    def test_removes_incomplete_blob_and_reports_freed_bytes(self, fake_hub):
        es_mx_dir = fake_hub / ES_MX_FOLDER
        incomplete = _write_blob(es_mx_dir, "abc123.incomplete", b"x" * 42)

        freed = purge_incomplete_downloads()

        assert freed == 42
        assert not incomplete.exists()

    def test_preserves_complete_blobs(self, fake_hub):
        es_mx_dir = fake_hub / ES_MX_FOLDER
        complete = _write_blob(es_mx_dir, "complete-hash", b"safetensors-content")
        incomplete = _write_blob(es_mx_dir, "def456.incomplete", b"y" * 10)

        freed = purge_incomplete_downloads()

        assert freed == 10
        assert complete.exists()
        assert not incomplete.exists()

    def test_purges_both_project_cache_dirs(self, fake_hub):
        es_mx_incomplete = _write_blob(fake_hub / ES_MX_FOLDER, "a.incomplete", b"1" * 5)
        base_incomplete = _write_blob(fake_hub / BASE_FOLDER, "b.incomplete", b"2" * 7)

        freed = purge_incomplete_downloads()

        assert freed == 12
        assert not es_mx_incomplete.exists()
        assert not base_incomplete.exists()

    def test_does_not_touch_incomplete_blobs_outside_project_dirs(self, fake_hub):
        """Solo se purgan las dos carpetas de model_cache_dirs(); otro repo
        cacheado en la misma raíz de HuggingFace queda intacto."""
        unrelated_incomplete = _write_blob(
            fake_hub / "models--Some--OtherRepo", "c.incomplete", b"z" * 99
        )

        freed = purge_incomplete_downloads()

        assert freed == 0
        assert unrelated_incomplete.exists()

    def test_multiple_incomplete_blobs_in_same_dir_all_removed(self, fake_hub):
        es_mx_dir = fake_hub / ES_MX_FOLDER
        first = _write_blob(es_mx_dir, "one.incomplete", b"a" * 3)
        second = _write_blob(es_mx_dir, "two.incomplete", b"bb" * 3)

        freed = purge_incomplete_downloads()

        assert freed == 9
        assert not first.exists()
        assert not second.exists()

    def test_missing_blobs_dir_is_skipped_without_error(self, fake_hub):
        """model_cache_dirs() existe pero sin subcarpeta blobs/: no debe fallar."""
        (fake_hub / ES_MX_FOLDER).mkdir(parents=True)
        assert purge_incomplete_downloads() == 0

    def test_unlink_oserror_is_tolerated_and_other_files_still_purged(self, fake_hub, monkeypatch):
        """Un archivo en uso (permiso denegado) no debe abortar la purga completa
        de los demás '.incomplete' pendientes."""
        es_mx_dir = fake_hub / ES_MX_FOLDER
        locked = _write_blob(es_mx_dir, "locked.incomplete", b"L" * 4)
        removable = _write_blob(es_mx_dir, "removable.incomplete", b"R" * 6)

        from pathlib import Path as PathType
        original_unlink = PathType.unlink

        def _flaky_unlink(self, *args, **kwargs):
            if self.name == "locked.incomplete":
                raise OSError("archivo en uso")
            return original_unlink(self, *args, **kwargs)

        monkeypatch.setattr(PathType, "unlink", _flaky_unlink)

        freed = purge_incomplete_downloads()

        assert freed == 6  # solo se contabiliza el que sí se pudo borrar
        assert locked.exists()
        assert not removable.exists()

    def test_only_matches_incomplete_suffix(self, fake_hub):
        """Un blob completo cuyo nombre contenga 'incomplete' en otra posición
        (no como sufijo exacto '*.incomplete') no debe coincidir con el glob."""
        es_mx_dir = fake_hub / ES_MX_FOLDER
        decoy = _write_blob(es_mx_dir, "incomplete-but-not-suffix.bin", b"decoy")
        real_incomplete = _write_blob(es_mx_dir, "real.incomplete", b"gone")

        freed = purge_incomplete_downloads()

        assert freed == len(b"gone")
        assert decoy.exists()
        assert not real_incomplete.exists()


class TestModelCacheDirs:
    def test_returns_exactly_the_two_project_repos(self, fake_hub):
        dirs = model_cache_dirs()
        names = {d.name for d in dirs}
        assert names == {ES_MX_FOLDER, BASE_FOLDER}

    def test_dirs_returned_even_when_absent_on_disk(self, fake_hub):
        """model_cache_dirs() no crea ni exige que las carpetas existan."""
        dirs = model_cache_dirs()
        assert len(dirs) == 2
        assert all(not d.exists() for d in dirs)


class TestModelAllowPatterns:
    """MODEL_ALLOW_PATTERNS contiene los archivos correctos por modelo."""

    def test_has_entries_for_all_models(self):
        for alias in MODELS:
            assert alias in MODEL_ALLOW_PATTERNS, (
                f"Falta MODEL_ALLOW_PATTERNS para el modelo '{alias}'"
            )

    def test_patterns_are_lists_of_strings(self):
        for alias, patterns in MODEL_ALLOW_PATTERNS.items():
            assert isinstance(patterns, list), f"Patterns para '{alias}' no es lista"
            assert len(patterns) > 0, f"Patterns para '{alias}' está vacío"
            for p in patterns:
                assert isinstance(p, str), f"Pattern para '{alias}' no es string: {p}"

    def test_es_mx_latam_patterns_match_required_files(self):
        patterns = MODEL_ALLOW_PATTERNS["es-mx-latam"]
        assert "t3_es_mx_latam.safetensors" in patterns
        assert "s3gen_v3.safetensors" in patterns
        assert "grapheme_mtl_merged_expanded_v1.json" in patterns

    def test_es_mx_latam_excludes_s3gen_pt_duplicate(self):
        """s3gen_v3.pt es el mismo checkpoint en formato duplicado (~1 GB):
        safetensors ya valida integridad en la carga, así que no se descarga."""
        assert "s3gen_v3.pt" not in MODEL_ALLOW_PATTERNS["es-mx-latam"]

    def test_en_patterns_match_required_files(self):
        patterns = MODEL_ALLOW_PATTERNS["en"]
        assert "t3_cfg.safetensors" in patterns
        assert "s3gen.safetensors" in patterns
        assert "ve.safetensors" in patterns
        assert "tokenizer.json" in patterns
        assert "conds.pt" in patterns
