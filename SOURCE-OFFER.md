# Disponibilidad del código fuente (MPL-2.0 §3.2)

**AI Voice-InterConnector 0.37.4** se distribuye bajo la licencia
**MPL-2.0** (ver `LICENSE`). Conforme a la sección 3.2 de la MPL-2.0,
el código fuente correspondiente a esta versión exacta está disponible en:

- **Código fuente (tarball del tag):**
  <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/archive/refs/tags/v0.37.4.tar.gz>
- **Release v0.37.4 (artefactos y notas):**
  <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.37.4>
- **Repositorio:** <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector>

Las atribuciones de las dependencias redistribuidas están en
`THIRD-PARTY-LICENSES.md`, junto a este archivo. Los pesos de los modelos no se
empaquetan en el binario: se descargan con `setup` y conservan sus licencias
(Qwen3-TTS MIT/Apache-2.0, opus-mt CC-BY-4.0).

El paquete incluye **FFmpeg 9.0.2** como ejecutable independiente: la aplicación
lo invoca como proceso y no lo enlaza con el binario principal. Los builds
distribuidos son GPL-3.0-or-later. Su código fuente correspondiente está
disponible en:

- **FFmpeg 9.0.2 (fuente upstream):**
  <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>
- **Builds Linux (BtbN) y scripts de compilación:**
  <https://github.com/BtbN/FFmpeg-Builds/releases/tag/autobuild-2026-10-24-13-02>
  (build `n9.0.2-35-g9a9b70bb8c`)
- **Build Windows (Gyan):**
  <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2>
  (el mismo binario se publica en <https://www.gyan.dev/ffmpeg/builds/>)
- **Build macOS (Martin Riedl):** <https://ffmpeg.martin-riedl.de/>

Si necesitas el árbol de fuentes exacto de un build, incluidas las bibliotecas
que enlaza, solicítalo abriendo un issue en el repositorio o contactando al
mantenedor. La oferta se mantiene durante al menos tres años desde la entrega
del binario.

Si recibiste este binario sin acceso a las URLs anteriores, puedes solicitar
el código fuente abriendo un issue en el repositorio o contactando al
mantenedor del proyecto, Cristián Rojas Arredondo.
