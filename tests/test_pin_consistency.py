"""Tests de consistencia de pines de versión y del slug del repo.

Fuentes únicas (diseño mixto YAML+Python):
  - **Pines de entorno-CI (Python, pytest)**: pipeline parameters en
    `.circleci/config.yml` (una sola declaración `default:`; el resto se
    interpola con `<< pipeline.parameters.NAME >>`). Este test falla si un
    literal de esas versiones reaparece fuera del bloque `parameters:` —es
    decir, si alguien se salta la fuente única.
  - **Pin de build de Inno Setup**: `scripts/build_utils.INNOSETUP_PIN` (lado
    Python). El `choco install innosetup` de `config.yml` debe coincidir.
  - **Slug del repo de GitHub**: `scripts/render_cask.GITHUB_REPO` (lado
    Python). Los consumidores heterogéneos (installers shell, config de CI,
    template de issues, installer de Windows) —que no pueden importar Python—
    se vigilan aquí contra deriva en vez de forzar una consolidación frágil
    entre lenguajes.

Convierte la «deriva silenciosa» de un pin duplicado en un fallo de CI.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import build_utils  # noqa: E402
import render_cask  # noqa: E402
import render_source_offer  # noqa: E402

CONFIG_TEXT = (ROOT / ".circleci" / "config.yml").read_text(encoding="utf-8")


def _param_default(name: str) -> str:
    """Extrae el `default:` de un pipeline parameter de config.yml."""
    m = re.search(
        rf'{name}:.*?default:\s*"([^"]+)"',
        CONFIG_TEXT,
        re.DOTALL,
    )
    assert m, f"no se encontró el pipeline parameter '{name}' con un default"
    return m.group(1)


class TestPipelineParameterSingleSource:
    """Los pines de entorno-CI se declaran una sola vez (pipeline parameter)."""

    def test_python_version_declared_once(self):
        version = _param_default("python_version")
        occurrences = CONFIG_TEXT.count(version)
        assert occurrences == 1, (
            f"la versión de Python {version!r} aparece {occurrences} veces en "
            "config.yml; debe declararse solo en el pipeline parameter e "
            "interpolarse con << pipeline.parameters.python_version >>"
        )

    def test_pytest_version_declared_once(self):
        version = _param_default("pytest_version")
        occurrences = CONFIG_TEXT.count(version)
        assert occurrences == 1, (
            f"la versión de pytest {version!r} aparece {occurrences} veces en "
            "config.yml; debe declararse solo en el pipeline parameter e "
            "interpolarse con << pipeline.parameters.pytest_version >>"
        )

    def test_pytest_cov_version_declared_once(self):
        version = _param_default("pytest_cov_version")
        occurrences = CONFIG_TEXT.count(version)
        assert occurrences == 1, (
            f"la versión de pytest-cov {version!r} aparece {occurrences} veces en "
            "config.yml; debe declararse solo en el pipeline parameter e "
            "interpolarse con << pipeline.parameters.pytest_cov_version >>"
        )

    def test_pins_are_interpolated_not_hardcoded(self):
        # Prueba de humo del mecanismo: las referencias interpoladas existen.
        assert "<< pipeline.parameters.python_version >>" in CONFIG_TEXT
        assert "<< pipeline.parameters.pytest_version >>" in CONFIG_TEXT
        assert "<< pipeline.parameters.pytest_cov_version >>" in CONFIG_TEXT


class TestInnoSetupPin:
    """El pin de Inno vive en build_utils.py; CI no debe divergir."""

    def test_ci_matches_build_utils(self):
        m = re.search(
            r"choco install innosetup[^\n]*--version=([0-9][0-9.]*)",
            CONFIG_TEXT,
        )
        assert m, "no se encontró 'choco install innosetup --version=...' en config.yml"
        assert m.group(1) == build_utils.INNOSETUP_PIN, (
            f"Inno Setup en config.yml ({m.group(1)}) != "
            f"build_utils.INNOSETUP_PIN ({build_utils.INNOSETUP_PIN}); "
            "actualiza la constante y el comando de choco a la vez."
        )


class TestGlibcFloorConsistency:
    """El piso glibc declarado en build_utils.GLIBC_FLOOR debe coincidir con el
    que valida install-linux.sh. Sin esto, el instalador y el AppImage divergen
    en el piso real y el binario falla en runtime en sistemas validados como
    compatibles."""

    def test_glibc_floor_matches_install_script(self):
        floor = build_utils.GLIBC_FLOOR
        assert isinstance(floor, tuple) and len(floor) == 2, (
            "build_utils.GLIBC_FLOOR debe ser una tupla (major, minor)"
        )
        major, minor = floor

        install_text = (ROOT / "install-linux.sh").read_text(encoding="utf-8")
        assert f"GLIBC_FLOOR_MAJOR={major}" in install_text, (
            f"install-linux.sh debe declarar GLIBC_FLOOR_MAJOR={major} "
            f"(coincidente con build_utils.GLIBC_FLOOR={floor}); el instalador "
            "y el AppImage divergerían en el piso glibc."
        )
        assert f"GLIBC_FLOOR_MINOR={minor}" in install_text, (
            f"install-linux.sh debe declarar GLIBC_FLOOR_MINOR={minor} "
            f"(coincidente con build_utils.GLIBC_FLOOR={floor}); el instalador "
            "y el AppImage divergerían en el piso glibc."
        )


class TestGitHubRepoSlug:
    """El slug del repo es único: render_cask.GITHUB_REPO. Vigilamos que cada
    consumidor lo referencie, sin forzar imports entre shell/YAML/Python."""

    # Archivos que hardcodean el slug del repo principal (no el tap de Homebrew,
    # que es CristianRojas-SoftwareEngineer/homebrew-ai-voice-interconnector, otro repo).
    REPO_CONSUMERS = (
        "install-linux.sh",
        "install-macos.sh",
        ".circleci/config.yml",
        "scripts/create_installer_windows.py",
        ".github/ISSUE_TEMPLATE/config.yml",
    )

    def test_canonical_slug_present_in_all_consumers(self):
        canonical = render_cask.GITHUB_REPO
        assert "/" in canonical, "GITHUB_REPO debe ser owner/repo"
        for rel in self.REPO_CONSUMERS:
            text = (ROOT / rel).read_text(encoding="utf-8")
            assert canonical in text, (
                f"{rel} no referencia el repo canónico {canonical!r} "
                "(fuente: render_cask.GITHUB_REPO); si el slug cambió, "
                "actualiza todos los consumidores a la vez."
            )


class TestSourceOfferVersion:
    """SOURCE-OFFER.md (oferta GPLv3 §6, commiteado en la raíz) debe ser
    byte-idéntico a la salida del generador para la versión actual: cubre a la
    vez el drift de versión (bump sin regenerar) y el drift de plantilla."""

    def test_committed_offer_matches_generator(self):
        committed = (ROOT / "SOURCE-OFFER.md").read_text(encoding="utf-8")
        expected = render_source_offer.render_source_offer(build_utils.get_version())
        assert committed == expected, (
            "SOURCE-OFFER.md diverge del generador para la versión "
            f"{build_utils.get_version()}; regenera con: "
            "python scripts/render_source_offer.py > SOURCE-OFFER.md"
        )
