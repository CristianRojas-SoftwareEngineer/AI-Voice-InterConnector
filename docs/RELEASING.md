# Publicación de una versión (RELEASING.md)

`ai-voice-interconnector` publica sus releases de forma **automática**: al pushear un
tag `v*`, CircleCI corre los tests, los 4 builds y el job `publish-release`, que
recolecta los artefactos, genera `SHA256SUMS.txt` y **publica el GitHub Release
directo** (sin borrador) sobre el tag. Sin firma de código, el cotejo de
checksums SHA-256 sigue siendo la cadena de verificación de integridad para el
usuario final.

El canal **PyPI fue retirado en la Fase 7** (ver [docs/DISTRIBUTION.md](DISTRIBUTION.md)):
desde la migración a Rust la distribución es 100% archivos comprimidos Rust y no
hay publicación a PyPI. Solo `publish-release` publica en firme en cada tag.

## Tabla de contenidos

- [Prerequisitos](#prerequisitos)
- [1. Corte: crear y publicar el tag](#1-corte-crear-y-publicar-el-tag)
- [2. Automático: lo que hace el CI](#2-automático-lo-que-hace-el-ci)
- [3. Verificación post-publicación](#3-verificación-post-publicación)
- [4. Verificación del usuario final](#4-verificación-del-usuario-final)

## Prerequisitos

- No hay hallazgos Bloqueantes ni Mayores abiertos (criterios de
  aceptación del release; ver `docs/GOAL.md` §"Criterios de Aceptación").
- `CHANGELOG.md` tiene la sección de la versión a publicar cortada (no
  "No publicado"), con las entradas reales de esa versión. **El job de release
  falla si no encuentra la sección `[X.Y.Z]`** (X.Y.Z = tag sin la `v`), así que
  este corte es obligatorio antes de taggear.
- `SOURCE-OFFER.md` (oferta de fuente GPLv3 §6, viaja dentro de los 4
  artefactos) está regenerado para la versión a publicar: tras el bump de
  `VERSION` (`src/main.rs`/`Cargo.toml`), correr `cargo run -p xtask -- source-offer >
  SOURCE-OFFER.md` y commitear el resultado antes del tag. El gate
  `validate-licenses` (`cargo run -p xtask -- source-offer --check`) falla
  si el archivo commiteado diverge del generador.
- `THIRD-PARTY-LICENSES.md` está en sincronía con `Cargo.lock` (Rust):
  ante altas/bajas de crates, regenerar el inventario (cargo-license) y
  verificar con `cargo run -p xtask -- licenses --check`. El gate
  `validate-licenses` falla ante faltantes/sobrantes.
- La suite pasa localmente (`cargo test --all`; incluye los tests de `xtask`) en el commit a taggear. **No
  hay forma de verificar esto en CircleCI antes de taggear**: el workflow
  `build-all` tiene `branches: ignore: /.*/` en todos sus jobs, así que
  CircleCI no corre nada en pushes a `main` — el tag es lo único que dispara
  el pipeline. La protección dentro del pipeline no es una comprobación
  previa, sino el propio grafo de dependencias: los 4 builds
  (`build-windows-x64`, `build-linux-x64`, `build-linux-arm64`,
  `build-darwin-arm64`) declaran `requires` sobre la triple puerta de tests
  (`test-linux`, `test-windows`, `test-macos`) **más** los tres smoke-tests de
  instaladores (`test-installer-linux`, `test-installer-windows`,
  `test-installer-macos`) **más** `coverage` y `validate-licenses`.
  Si cualquiera de esas puertas falla en el pipeline del tag, los builds no llegan a ejecutarse. Correr la suite en local antes de
  taggear sigue siendo la única manera de anticipar ese resultado.
- **Revisiones fijadas de los modelos auditadas**: los modelos Qwen3-TTS y opus-mt
  se descargan vía `ai-voice-interconnector setup` (no se empaquetan). Si el
  release debe incorporar una versión nueva de alguno de los modelos: consultar el `sha`
  vigente (`https://huggingface.co/api/models/<repo>`), auditar el diff de esa
  revisión en HF y verificar con `setup` + `doctor`.
- **Prerequisito operativo (una sola vez):** existe el context `github-release`
  en CircleCI (Organization Settings → Contexts) con la variable `GH_TOKEN` = un
  fine-grained PAT con permiso `contents: write` sobre el repo. Está aislado al
  job `publish-release`; ningún otro job lo ve.
- **Prerequisitos del canal Cask de macOS (una sola vez):** existe el
  repositorio tap `homebrew-ai-voice-interconnector` (público), y el context de CircleCI
  `homebrew-tap` con la variable `HOMEBREW_TAP_PAT` (un PAT fine-grained con
  permiso `Contents:RW` solo sobre el tap). Está aislado al job
  `publish-metadata`. Además, **el primer Cask del tap es un bootstrap manual
  único**: `publish-metadata` reescribe `Casks/ai-voice-interconnector.rb`, pero asume que
  el archivo ya existe en el tap la primera vez (crearlo a mano con el
  generador: `cargo run -p xtask -- cask --tag vX.Y.Z --sums-file
  SHA256SUMS.txt --out Casks/ai-voice-interconnector.rb` y commitear/pushear al tap antes
  del primer release que dependa de este job). Detalle del diseño completo en
  [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).
- **La publicación del Release es irreversible**: se publica directo sobre el tag y, para revertirlo, hay que borrar un Release
  ya público. Por eso el corte del `CHANGELOG.md` y la versión en `src/main.rs`/`Cargo.toml` deben estar
  correctos **antes** de crear el tag, no después.

## 1. Corte: crear y publicar el tag

```bash
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

El push del tag dispara el workflow `build-all` en CircleCI sobre ese commit:
triple puerta de tests + 3 smoke-tests de instaladores + 4 builds **y además**
el job `publish-release` (que solo corre en tags `v*`, nunca en ramas).

## 2. Automático: lo que hace el CI

Una vez pushado el tag, el pipeline ejecuta sin intervención:

1. **Tests + builds**: la triple puerta de tests, los smoke-tests de
   instaladores (`test-installer-linux`, `test-installer-windows`,
   `test-installer-macos`) y los 4
   builds nativos (`build-windows-x64`, `build-linux-x64`, `build-linux-arm64`,
   `build-darwin-arm64`). Cada build emite el SHA-256 de su artefacto en el log
   (step "Emit artifact SHA-256") y **persiste el artefacto versionado** al
   workspace compartido.
2. **`publish-release`** (tras los 4 builds):
   - Recolecta los 4 artefactos del workspace (`attach_workspace`) — ya con su
     nombre de release: `ai-voice-interconnector-X.Y.Z-x86_64-windows.zip`,
     `ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz`, `ai-voice-interconnector-X.Y.Z-arm64-linux.tar.gz`,
     `ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz`.
   - Genera `SHA256SUMS.txt` con los checksums de los 4.
   - Extrae las notas de la sección `[X.Y.Z]` de `CHANGELOG.md`.
   - Inyecta en `notes.md` (tras el recorte del CHANGELOG) un pie con la oferta
     de código fuente **GPLv3 §6**: el tarball del tag
     (`archive/refs/tags/vX.Y.Z.tar.gz`) y el enlace al tag, para que todo
     release cumpla el compliance de fuente de forma automática sin depender de
     que el humano la añada a mano.
   - Publica el GitHub Release directo (sin borrador) sobre el tag `vX.Y.Z`, con
     los 5 assets (4 artefactos + `SHA256SUMS.txt`) y las notas.
 3. **`publish-metadata`** (tras `publish-release`, solo en tags `v*`): recupera
   `SHA256SUMS.txt` del Release recién publicado (`gh release download`,
   idempotente y sin depender del workspace del pipeline), reescribe
   `Casks/ai-voice-interconnector.rb` con la versión del tag y el sha256 del `.dmg` arm64
   (`cargo xtask cask`), y hace push al tap `homebrew-ai-voice-interconnector`. Si el
   Cask no cambia (regeneración con los mismos inputs), el commit es un no-op y
   no se empuja nada; reintentar este job en cualquier momento es seguro.

Ya no hay descarga ni cotejo manual de artefactos: la recolección por workspace
es determinista (el mismo binario que pasó el smoke test es el que se adjunta).

## 3. Verificación post-publicación

En la pestaña **Releases** del repo aparece el Release `vX.Y.Z` **ya público**
(marcado como *latest* por defecto), con sus 5 assets y las notas del CHANGELOG.
Verifica:

- Los **5 assets** están presentes (4 artefactos + `SHA256SUMS.txt`).
- Las **notas** corresponden a la sección `[X.Y.Z]` del `CHANGELOG.md`.
- Las **notas** incluyen el pie de oferta de código fuente GPLv3 §6: el enlace al
  tarball de fuente (`.../archive/refs/tags/vX.Y.Z.tar.gz`) aparece en el Release.
- Opcional: coteja los hashes de `SHA256SUMS.txt` contra los emitidos por cada
  build en el log del pipeline (step "Emit artifact SHA-256").

El tag es el **punto de no retorno**: el Release es público en cuanto el job
termina. Si algo falla (faltan assets, notas incorrectas, artefacto roto), la
iteración ya no es descartar un borrador: **borra el Release público** y
**re-crea el tag** sobre el commit corregido (p. ej. tras arreglar la sección del
CHANGELOG).

> Si el tag ya tuviera un Release, `gh release create` falla ruidosamente en el
> CI (no re-publica en silencio): borra el Release anterior antes de re-taggear.

## 4. Verificación del usuario final

El usuario final puede verificar la integridad de su descarga contra
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
