#!/usr/bin/env python3
"""Generador puro del Cask de Homebrew `tts-sidecar.rb`.

Reescribe el contenido del Cask a partir de un tag de versión (`CIRCLE_TAG`,
p. ej. "v1.2.3") y el contenido de `SHA256SUMS.txt` publicado junto al
Release (fuente del hash del `.dmg` de macOS arm64). Sin I/O de red: toma el
texto de `SHA256SUMS.txt` ya descargado (por el job `publish-metadata` en CI,
o por el test) y devuelve el Ruby del Cask como string.

Reutilizado por el job `publish-metadata` de `.circleci/config.yml` (que lo
invoca tras cada `publish-release`) y por `tests/test_cask.py`.
"""

import re
import sys
from pathlib import Path

# Nombre del Cask en el tap `homebrew-tts-sidecar` (Casks/tts-sidecar.rb).
CASK_NAME = "tts-sidecar"

# Owner/repo de GitHub, fuente de la URL de descarga del .dmg y de livecheck.
GITHUB_REPO = "CristianRojas-SoftwareEngineer/TTS-Sidecar"

_CASK_TEMPLATE = """\
cask "{cask_name}" do
  version "{version}"
  sha256 "{sha256}"

  url "https://github.com/{repo}/releases/download/v#{{version}}/tts-sidecar-#{{version}}-arm64.dmg"
  name "TTS Sidecar"
  desc "Motor de sintesis de voz (TTS) offline con clonacion de voz en espanol latinoamericano"
  homepage "https://github.com/{repo}"

  livecheck do
    url :url
    strategy :github_latest
  end

  depends_on macos: ">= :big_sur"

  app "tts-sidecar-arm64.app"
  binary "#{{appdir}}/tts-sidecar-arm64.app/Contents/MacOS/tts-sidecar"

  zap trash: [
    "~/Library/Application Support/tts-sidecar",
    "~/.cache/huggingface/hub/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam",
    "~/.cache/huggingface/hub/models--ResembleAI--chatterbox",
  ]

  caveats <<~EOS
    El modelo de voz (es-mx-latam, ~4 GB) no viene incluido:
    descargalo una sola vez con:
      tts-sidecar setup

    Licencia: GPL-3.0-or-later. La oferta de codigo fuente (GPLv3 seccion 6)
    y las atribuciones de terceros viajan dentro del bundle instalado:
      #{{appdir}}/tts-sidecar-arm64.app/Contents/Resources/SOURCE-OFFER.md
      #{{appdir}}/tts-sidecar-arm64.app/Contents/Resources/THIRD-PARTY-LICENSES.md
  EOS
end
"""


def parse_dmg_sha256(sums_text: str, version: str) -> str:
    """Extrae el SHA-256 del .dmg de macOS arm64 de un SHA256SUMS.txt.

    `SHA256SUMS.txt` tiene una línea por artefacto: `<hash>  <nombre-archivo>`
    (formato de `sha256sum`). Busca la línea del .dmg versionado
    (`tts-sidecar-<version>-arm64.dmg`, sin la `v` inicial del tag) y devuelve
    su hash. Aborta con ValueError si no la encuentra o si el archivo tiene
    varias coincidencias (ambigüedad que nunca debería ocurrir).
    """
    pattern = re.compile(
        r"^([0-9a-fA-F]{64})\s+\S*tts-sidecar-" + re.escape(version) + r"-arm64\.dmg\s*$",
        re.MULTILINE,
    )
    matches = pattern.findall(sums_text)
    if not matches:
        raise ValueError(
            f"No se encontró el hash del .dmg arm64 de la versión {version} en SHA256SUMS.txt"
        )
    if len(matches) > 1:
        raise ValueError(
            f"Múltiples líneas coinciden con el .dmg arm64 de la versión {version} en SHA256SUMS.txt"
        )
    return matches[0].lower()


def render_cask(version: str, sha256: str) -> str:
    """Genera el contenido Ruby del Cask para una versión y un sha256 dados.

    `version` es la versión sin la `v` inicial del tag (p. ej. "1.2.3").
    """
    return _CASK_TEMPLATE.format(
        cask_name=CASK_NAME,
        version=version,
        sha256=sha256,
        repo=GITHUB_REPO,
    )


def render_cask_from_tag(circle_tag: str, sums_text: str) -> str:
    """Combina el tag de CI y SHA256SUMS.txt en el Cask final.

    `circle_tag` es el tag completo con la `v` (p. ej. "v1.2.3", el valor de
    la variable de entorno CIRCLE_TAG); se despoja la `v` para la stanza
    `version` del Cask (convención de Homebrew) y para localizar el .dmg en
    SHA256SUMS.txt (el nombre del artefacto no lleva la `v`).
    """
    version = circle_tag.lstrip("v")
    sha256 = parse_dmg_sha256(sums_text, version)
    return render_cask(version, sha256)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Genera Casks/tts-sidecar.rb")
    parser.add_argument("--tag", required=True, help="Tag de release, p. ej. v1.2.3 (CIRCLE_TAG)")
    parser.add_argument("--sums-file", required=True, type=Path, help="Ruta a SHA256SUMS.txt")
    parser.add_argument("--out", required=True, type=Path, help="Ruta de salida del Cask .rb")
    args = parser.parse_args()

    sums_text = args.sums_file.read_text(encoding="utf-8")
    cask_content = render_cask_from_tag(args.tag, sums_text)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(cask_content, encoding="utf-8")
    print(f"Cask generado: {args.out}", file=sys.stderr)
