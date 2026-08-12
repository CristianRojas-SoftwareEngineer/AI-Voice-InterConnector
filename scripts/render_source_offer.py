#!/usr/bin/env python3
"""Generador puro de `SOURCE-OFFER.md` (oferta de código fuente, GPLv3 §6).

La oferta escrita debe acompañar a cada artefacto binario distribuido: si
solo viviera en las Release notes de GitHub, quien recibe el binario por otra
vía (copia directa, mirror, one-liner) recibiría el objeto sin la oferta que
la licencia exige. Este script renderiza la oferta desde la versión
single-source (`build_utils.get_version()`) y el slug canónico del repo
(`render_cask.GITHUB_REPO`); su salida se commitea como `SOURCE-OFFER.md` en
la raíz y viaja dentro de los 4 artefactos (bundles nativos vía
`LICENSE_FILES`, wheel/sdist vía `license-files` de pyproject.toml).

`tests/test_pin_consistency.py::TestSourceOfferVersion` compara byte a byte
el archivo commiteado contra esta plantilla: un bump de versión sin regenerar
el archivo rompe la suite. Regenerar con:

    python scripts/render_source_offer.py > SOURCE-OFFER.md

Complementa (no reemplaza) la oferta que `publish-release` inyecta en las
Release notes de cada tag.
"""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from build_utils import get_version
from render_cask import GITHUB_REPO

_SOURCE_OFFER_TEMPLATE = """\
# Oferta de código fuente (GPLv3 §6)

**AI Voice InterConnector {version}** se distribuye bajo la licencia
**GPL-3.0-or-later** (ver `LICENSE`). Conforme a la sección 6 de la GPLv3,
este binario va acompañado de una oferta de acceso al código fuente completo
correspondiente a esta versión exacta:

- **Código fuente (tarball del tag):**
  <https://github.com/{repo}/archive/refs/tags/v{version}.tar.gz>
- **Release v{version} (artefactos y notas):**
  <https://github.com/{repo}/releases/tag/v{version}>
- **Repositorio:** <https://github.com/{repo}>

Las atribuciones de las dependencias redistribuidas están en
`THIRD-PARTY-LICENSES.md`, junto a este archivo. El modelo de voz Chatterbox
no se empaqueta en el binario y se licencia por separado (MIT, ResembleAI).

Si recibiste este binario sin acceso a las URLs anteriores, puedes solicitar
el código fuente abriendo un issue en el repositorio o contactando al
mantenedor del proyecto, Cristián Rojas Arredondo.
"""


def render_source_offer(version: str, repo: str = GITHUB_REPO) -> str:
    """Genera el contenido Markdown de la oferta de fuente para una versión."""
    return _SOURCE_OFFER_TEMPLATE.format(version=version, repo=repo)


if __name__ == "__main__":
    # UTF-8 explícito: en Windows, stdout redirigido a archivo usa cp1252 por
    # defecto y corrompería los acentos del Markdown generado.
    sys.stdout.reconfigure(encoding="utf-8")
    print(render_source_offer(get_version()), end="")
