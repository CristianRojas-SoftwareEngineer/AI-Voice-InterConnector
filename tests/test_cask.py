"""Tests del generador puro del Cask de Homebrew (scripts/render_cask.py).

Patrón de tests/test_build_windows.py: sin I/O de red, monkeypatch/fixtures
puras. Dado un CIRCLE_TAG y un SHA256SUMS.txt de ejemplo, verifica que las
stanzas version/sha256/url del Cask se reescriben correctamente.
"""

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))
from render_cask import parse_dmg_sha256, render_cask, render_cask_from_tag

def _sha(label: str) -> str:
    import hashlib
    return hashlib.sha256(label.encode()).hexdigest()


DMG_SHA = _sha("dmg")

SAMPLE_SUMS = f"""\
{_sha("exe")}  ai-voice-interconnector-1.2.3-x86_64-setup.exe
{_sha("appimage-x86_64")}  ai-voice-interconnector-1.2.3-x86_64.AppImage
{_sha("appimage-arm64")}  ai-voice-interconnector-1.2.3-arm64.AppImage
{DMG_SHA}  ai-voice-interconnector-1.2.3-arm64.dmg
"""


class TestParseDmgSha256:
    def test_extracts_hash_of_matching_dmg(self):
        assert parse_dmg_sha256(SAMPLE_SUMS, "1.2.3") == DMG_SHA

    def test_missing_version_raises(self):
        with pytest.raises(ValueError):
            parse_dmg_sha256(SAMPLE_SUMS, "9.9.9")

    def test_does_not_match_other_artifacts(self):
        # No debe confundirse con el .exe o los .AppImage de la misma versión.
        result = parse_dmg_sha256(SAMPLE_SUMS, "1.2.3")
        assert result != _sha("exe")
        assert result != _sha("appimage-x86_64")


class TestRenderCask:
    def test_stanzas_reflect_version_and_sha256(self):
        content = render_cask("1.2.3", DMG_SHA)
        assert 'version "1.2.3"' in content
        assert f'sha256 "{DMG_SHA}"' in content
        assert "ai-voice-interconnector-#{version}-arm64.dmg" in content
        assert 'cask "ai-voice-interconnector" do' in content

    def test_url_points_to_versioned_tag(self):
        content = render_cask("1.2.3", DMG_SHA)
        assert "releases/download/v#{version}/" in content

    def test_zap_trash_present_for_clean_uninstall(self):
        content = render_cask("1.2.3", DMG_SHA)
        assert "zap trash:" in content

    def test_zap_trash_lists_both_model_repos(self):
        # La desinstalación con residuo cero requiere borrar los dos repos que
        # `setup` descarga: el modelo multilingüe y el repo base `chatterbox`
        # (Voice Encoder, ve.safetensors).
        content = render_cask("1.2.3", DMG_SHA)
        assert (
            "~/.cache/huggingface/hub/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam"
            in content
        )
        assert "~/.cache/huggingface/hub/models--ResembleAI--chatterbox" in content

    def test_caveats_suggest_setup(self):
        content = render_cask("1.2.3", DMG_SHA)
        assert "ai-voice-interconnector setup" in content

    def test_caveats_inform_license_and_source_offer(self):
        # La stanza `license` no existe en el DSL de Casks (deprecada sin
        # reemplazo en 2016, hoy exclusiva de fórmulas): la licencia y la
        # ubicación de la oferta de fuente GPLv3 §6 se informan vía caveats.
        content = render_cask("1.2.3", DMG_SHA)
        assert "GPL-3.0-or-later" in content
        assert "Contents/Resources/SOURCE-OFFER.md" in content
        assert "Contents/Resources/THIRD-PARTY-LICENSES.md" in content


class TestRenderCaskFromTag:
    def test_strips_leading_v_from_tag(self):
        content = render_cask_from_tag("v1.2.3", SAMPLE_SUMS)
        assert 'version "1.2.3"' in content
        assert f'sha256 "{DMG_SHA}"' in content

    def test_regenerating_from_same_inputs_is_idempotent(self):
        first = render_cask_from_tag("v1.2.3", SAMPLE_SUMS)
        second = render_cask_from_tag("v1.2.3", SAMPLE_SUMS)
        assert first == second
