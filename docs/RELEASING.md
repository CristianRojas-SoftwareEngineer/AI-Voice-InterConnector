# Publicación de una versión (RELEASING.md)

`ai-voice-interconnector` publica sus releases de forma **automática**. Al pushear un
tag `v*`, CircleCI ejecuta el pipeline `build-all`:

1. Corre las puertas: tests, cobertura, instaladores, licencias y CHANGELOG.
2. Compila los 4 artefactos nativos.
3. Publica el GitHub Release directamente sobre el tag, sin borrador, con los
   artefactos y `SHA256SUMS.txt`.
4. Actualiza el Cask de Homebrew.

La distribución es solo de archivos comprimidos con el binario Rust (ver
[docs/DISTRIBUTION.md](DISTRIBUTION.md)). Los binarios no están firmados, así que
la integridad se verifica cotejando los checksums SHA-256.

## Tabla de contenidos

- [Prerrequisitos](#prerrequisitos)
  - [Configuración única](#configuración-única)
  - [Antes de cada corte](#antes-de-cada-corte)
- [1. Corte: crear y publicar el tag](#1-corte-crear-y-publicar-el-tag)
- [2. Automático: lo que hace el CI](#2-automático-lo-que-hace-el-ci)
- [3. Verificación post-publicación](#3-verificación-post-publicación)
- [4. Verificación del usuario final](#4-verificación-del-usuario-final)

## Prerrequisitos

### Configuración única

- **Context `github-release`** en CircleCI (Organization Settings → Contexts).
  Contiene la variable `GH_TOKEN`, un PAT fine-grained con permiso
  `contents: write` sobre el repo. Lo usan solo `publish-release` y
  `publish-metadata`.
- **Context `homebrew-tap`** en CircleCI. Contiene la variable
  `HOMEBREW_TAP_PAT`, un PAT fine-grained con permiso `Contents:RW` solo sobre el
  repositorio tap público `homebrew-ai-voice-interconnector`. Lo usa solo
  `publish-metadata`, que en cada release crea o reescribe
  `Casks/ai-voice-interconnector.rb` en el tap. El diseño del canal está en
  [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

### Antes de cada corte

- **Sección `## [No publicado]` curada en `CHANGELOG.md`.** Se escribe a mano
  mientras avanza el desarrollo (Keep a Changelog) y no conserva marcadores
  `TODO: curar`. El corte no genera contenido: **promueve** esa sección a la
  versión que se publica.
- **Árbol de trabajo limpio y al menos un commit desde el último tag.** La
  curación del CHANGELOG también debe estar commiteada; `xtask release` aborta
  en cualquier otro caso.
- **`THIRD-PARTY-LICENSES.md` en sincronía con `Cargo.lock`.** Cada vez que
  cambia `Cargo.lock`, hay que regenerar el inventario con
  `cargo run -p xtask -- licenses` y revisar el diff. La comprobación
  `cargo run -p xtask -- licenses --check` compara el inventario completo:
  nombre, versión, licencia y familia de cada fila, y los totales.
- **Revisiones de los modelos auditadas.** Los modelos Qwen3-TTS y opus-mt se
  descargan con `ai-voice-interconnector setup` y no se empaquetan. Para
  incorporar una revisión nueva de alguno:
  1. consultar el `sha` vigente en `https://huggingface.co/api/models/<repo>`;
  2. auditar el diff de esa revisión en HuggingFace;
  3. verificarla con `setup` y `doctor`.
- **La publicación es irreversible.** El Release se publica directamente sobre
  el tag. La versión y el CHANGELOG deben estar correctos **antes** de crear el
  tag, porque corregirlos después exige borrar un Release público.

## 1. Corte: crear y publicar el tag

El repo es **trunk-based sobre `main`**. El trabajo diario y el corte ocurren en
`main`, y el tag `v*` se crea sobre `main`. El push del tag es el único
disparador de CI.

```bash
cargo run -p xtask -- release X.Y.Z   # o /release X.Y.Z (skill orquestadora)
cargo test --all                      # recomendado: la triple puerta de CI lo exige
git add -A
git commit -m "release: vX.Y.Z"      # conventional-commits
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin main --tags           # sin --tags el tag no dispara build-all
```

`cargo run -p xtask -- release X.Y.Z` hace el corte completo:

1. **Pre-validación.** Aborta sin tocar archivos si alguna de estas condiciones
   falla:
   - la versión tiene formato `X.Y.Z`;
   - `THIRD-PARTY-LICENSES.md` está en sincronía con `Cargo.lock`;
   - el código pasa `cargo fmt --all --check`;
   - `cargo clippy --all-targets -- -D warnings` no reporta avisos;
   - el árbol está limpio;
   - hay commits desde el último tag.
2. **Bump.** Escribe `X.Y.Z` en `src/main.rs`, `Cargo.toml`, `Cargo.lock`,
   `tests/golden/cli_version.json` y `SOURCE-OFFER.md`. `SOURCE-OFFER.md` es la
   oferta de código fuente GPLv3 §6 que viaja dentro de los 4 artefactos.
   Después regenera el inventario de `THIRD-PARTY-LICENSES.md`, que incluye la
   versión del propio crate.
3. **Promoción.** Convierte `## [No publicado]` de `CHANGELOG.md` en
   `## [X.Y.Z] — AAAA-MM-DD` y reemplaza su entrada del índice por una que apunta
   al ancla que GitHub asigna a la cabecera. También agrega la definición del
   enlace de comparación con el tag anterior. Falla si no existe
   `## [No publicado]`, si quedan marcadores `TODO: curar` o si `## [X.Y.Z]` ya
   existe.
4. **Post-comprobación.** Verifica `THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`
   y el CHANGELOG completo con las mismas funciones que usan las puertas de CI. Si algo falla en este paso,
   los archivos ya quedaron modificados: corrígelos a mano o revierte con
   `git checkout .` y reintenta.

Antes de commitear, revisa el diff, en especial la sección promovida del
CHANGELOG.

## 2. Automático: lo que hace el CI

El pipeline `build-all` corre solo en tags `v*` (`branches: ignore: /.*/`). La
arquitectura completa está en [docs/BUILD.md §4](BUILD.md#4-cicd-con-circleci).
Con el tag pusheado, ejecuta sin intervención:

1. **Puertas.** Son 9 y todas son `requires:` de los 4 builds. Si una falla, no
   se compila ni se publica nada.

   | Puerta | Qué comprueba |
   |---|---|
   | `test-linux`, `test-windows`, `test-macos` | `cargo test --all` en cada SO nativo |
   | `coverage` | Cobertura de la suite |
   | `test-installer-linux`, `test-installer-windows`, `test-installer-macos` | Smoke tests de los instaladores |
   | `validate-licenses` | `SOURCE-OFFER.md` coincide con su render para la versión del tag (`source-offer --check`) y `THIRD-PARTY-LICENSES.md` está en sincronía con `Cargo.lock` (`licenses --check`) |
   | `validate-changelog` | La promoción está completa (`changelog --check`): cabecera `[X.Y.Z]`, entrada del índice con el ancla de GitHub, enlace de comparación, sin `TODO: curar` y sin `[No publicado]` |

2. **Builds.** Son `build-windows-x64`, `build-linux-x64`, `build-linux-arm64`
   y `build-darwin-arm64`. Cada uno:
   - empaqueta su artefacto con el nombre del release;
   - emite su SHA-256 en el log, en el step «Emitir SHA-256 del artefacto»;
   - lo persiste en el workspace compartido.

   Los artefactos son:
   - `ai-voice-interconnector-X.Y.Z-x86_64-windows.zip`
   - `ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz`
   - `ai-voice-interconnector-X.Y.Z-arm64-linux.tar.gz`
   - `ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz`
3. **`publish-release`** (después de los 4 builds):
   - Toma los 4 artefactos del workspace. El binario que se adjunta es el
     mismo que pasó las puertas.
   - Genera `SHA256SUMS.txt`.
   - Extrae de `CHANGELOG.md` la sección `[X.Y.Z]` como notas. Si no la
     encuentra, falla.
   - Agrega a las notas un pie con la oferta de código fuente GPLv3 §6: el
     tarball del tag (`archive/refs/tags/vX.Y.Z.tar.gz`) y el enlace al tag.
   - Publica el GitHub Release sobre `vX.Y.Z` con 5 assets (4 artefactos y
     `SHA256SUMS.txt`) y las notas. Si el tag ya tiene un Release,
     `gh release create` falla.
4. **`publish-metadata`** (después de `publish-release`):
   - Descarga `SHA256SUMS.txt` del Release publicado.
   - Genera `Casks/ai-voice-interconnector.rb` con la versión del tag y el
     sha256 del `tar.gz` de macOS (`cargo run -p xtask -- cask`).
   - Lo empuja al tap `homebrew-ai-voice-interconnector`.

   Si el Cask no cambia, no empuja nada, así que se puede reintentar sin riesgo.

## 3. Verificación post-publicación

```bash
gh release view vX.Y.Z --json tagName,assets
```

En la pestaña **Releases** aparece `vX.Y.Z` ya público y marcado como *latest*.
Verifica:

- Los **5 assets** están presentes (4 artefactos y `SHA256SUMS.txt`).
- Las **notas** corresponden a la sección `[X.Y.Z]` del `CHANGELOG.md` e
  incluyen el pie de oferta de código fuente GPLv3 §6 con el enlace al tarball
  (`.../archive/refs/tags/vX.Y.Z.tar.gz`).
- Opcional: los hashes de `SHA256SUMS.txt` coinciden con los que cada build
  emitió en el log del pipeline.

**Recuperación ante fallas:**

- **Falla una puerta o un build.** No se publicó nada. Borra el tag
  (`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`), corrige y vuelve a
  crear el tag sobre el commit corregido.
- **El Release publicado tiene un defecto.** Borra el Release
  (`gh release delete vX.Y.Z --yes`) y el tag, corrige y vuelve a crear el tag.

## 4. Verificación del usuario final

El usuario final verifica la integridad de su descarga contra el
`SHA256SUMS.txt` publicado en el Release:

```bash
# Linux/macOS
sha256sum -c SHA256SUMS.txt --ignore-missing

# Windows (PowerShell)
Get-FileHash ai-voice-interconnector-X.Y.Z-x86_64-windows.zip -Algorithm SHA256
# comparar manualmente contra la línea correspondiente de SHA256SUMS.txt
```

Ver también `SECURITY.md` para el modelo de amenaza y la nota sobre binarios
sin firmar.
