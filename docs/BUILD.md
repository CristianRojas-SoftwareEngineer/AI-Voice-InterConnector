# Guía de Construcción

`ai-voice-interconnector` se compila como un **binario Rust autocontenido**
(`cargo build --release --features full`; CTranslate2 (ct2rs) compilado estático +
Parakeet vía ort load-dynamic (sin enlace en build)) y se **empaqueta por SO en un
archivo comprimido** (`tar.gz` en los 3 targets Unix, `.zip` en Windows) que
agrupa el binario con los documentos de licencia GPLv3.

## Tabla de contenidos

- [1. Requisitos](#1-requisitos)
- [2. Plataformas Soportadas](#2-plataformas-soportadas)
- [3. Compilación Local](#3-compilación-local)
- [4. CI/CD con CircleCI](#4-cicd-con-circleci)
- [5. Distribución de artefactos](#5-distribución-de-artefactos)
- [9. Build nativo del motor TTS (Rust/qwen_tts)](#9-build-nativo-del-motor-tts-rustqwen_tts)

---

## 1. Requisitos

- **Rust 1.96.0** (ver `rust_version` en `.circleci/config.yml` y `rust-toolchain.toml` si existe)
- **Cargo** (incluido con Rust)
- **CMake ≥ 3.20** (para CTranslate2/ct2rs, solo con `native-translation`)
- **pkg-config**
- **libasound2-dev** (Linux, para `cpal` ALSA) y **libclang-dev** (solo con `--features native-translation`/`full`, para `bindgen` de `ct2rs`; no requerido para `featureless` ni `native-stt`)
- **sccache 0.8.2** (opcional, acelera recompilaciones; el CI lo usa con `RUSTC_WRAPPER=sccache`)

No se requiere Python, Node ni toolchain adicional para compilar o empaquetar.

### Empaquetado por plataforma (archivos comprimidos)

El binario Rust es autocontenido (`crt-static`; CTranslate2 (ct2rs) enlazado
estático; Parakeet vía ort load-dynamic), así que el empaquetado **no requiere
herramientas de terceros**: cada target se comprime con una utilidad del sistema base.

| Plataforma | Formato | Utilidad de empaquetado |
|------------|---------|-------------------------|
| Linux x64 / ARM64 | `tar.gz` | `tar -czf` (coreutils) |
| macOS arm64 | `tar.gz` | `tar -czf` (base del SO) |
| Windows x64 | `.zip` | `Compress-Archive` (PowerShell) |

El step **«Preparar artefacto versionado (staging)»** de cada `build-*` en
`.circleci/config.yml` valida la versión (`const VERSION` de `src/main.rs` vs
`CIRCLE_TAG`, fail-fast), monta un directorio de staging con **layout plano**
—el binario renombrado a `ai-voice-interconnector[.exe]` (sin sufijo de
arquitectura) más los 4 documentos de la raíz (`LICENSE`,
`THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`, `README.md`), el `ort-bundle`
(ONNX Runtime + DLLs VC++ en Windows) y el `qwen_tts` vendido— y lo comprime al
archivo del target. Los documentos GPLv3 viajan así **dentro del archivo** y
quedan instalados junto al binario (cumplimiento §6 de la GPLv3 sin depender de
un bundle).

---

## 2. Plataformas Soportadas

| Plataforma | Compilación | Artefacto (archivo comprimido) |
|------------|-------------|-------------------------------|
| Windows x64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-x86_64-windows.zip` |
| Linux x64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-x86_64-linux.tar.gz` |
| Linux ARM64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-arm64-linux.tar.gz` |
| macOS arm64 (Apple Silicon) | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-arm64-macos.tar.gz` |

> **Por qué Linux publica 2 arquitecturas y Windows/macOS solo 1.** Cada
> plataforma publica las arquitecturas que cumplen **a la vez** dos condiciones:
> (a) población real de usuarios y (b) capacidad del toolchain Rust (sin
> dependencia de wheels Python). Bajo ese criterio:
>
> - **Windows → 1 (x86_64)** por **decisión**: Windows-on-ARM es marginal en la
>   población objetivo.
> - **macOS → 1 (arm64)** por **imposibilidad técnica heredada**: el toolchain
>   previo (torch) no publicaba wheels x86_64; en Rust se mantiene por paridad
>   y coste de runners Intel.
> - **Linux → 2 (x86_64 + arm64)** porque **ambas** arquitecturas tienen usuarios
>   y runners nativos (`arm.medium`).

### Matriz de arquitecturas y brechas

| SO | x86_64 | arm64 | Artefacto |
|----|:---:|:---:|----------|
| Windows | ✅ | ❌ | `.zip` (x64) |
| Linux | ✅ | ✅ | `tar.gz` x64 + `tar.gz` arm64 |
| macOS | ❌ | ✅ | `tar.gz` arm64 (Apple Silicon) |

**Arquitecturas faltantes y su justificación:**

- **Windows en ARM64:** no soportado por decisión de alcance (población marginal).
- **macOS Intel (x86_64):** no soportado; se acepta como limitación permanente.

### Matriz de SO probados y mínimos declarados

| SO | Probado en (CI) | Mínimo declarado | Origen del mínimo |
|----|-----------------|------------------|-------------------|
| Windows | Executor `circleci/windows@5.0` (Windows Server 2022) | Windows 10+ x64 | Binario Rust autocontenido, sin APIs posteriores |
| Linux | `cimg/rust:1.96.0` (base Ubuntu 22.04, glibc 2.35) | glibc ≥ 2.35 | Binario `gnu` (crt-static no cubre glibc); `install-linux.sh` advierte por debajo |
| macOS | Runner Apple Silicon Xcode 26.4 | macOS 13+ (Ventura) | Binario Rust arm64 |

**Limitación aceptada:** los mínimos declarados **no** se prueban en máquinas
con esas versiones exactas. Un reporte de fallo reabriría la decisión.

---

## 3. Compilación Local

### Verificación rápida

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all
cargo test --all --verbose   # suite completa (≈70 tests en cli_golden)
```

### Build de distribución (binario autocontenido)

```bash
# Binario featureless (sin STT/traducción, rápido, sin C++)
cargo build --release

# Binario completo (distribución, con STT + traducción)
cargo build --release --features full

# Ejecutar el binario recién compilado
./target/release/ai-voice-interconnector version
./target/release/ai-voice-interconnector voice list
./target/release/ai-voice-interconnector setup
./target/release/ai-voice-interconnector doctor
```

### ONNX Runtime vía `load-dynamic` (sin build en compilación)

El motor STT Parakeet consume **ONNX Runtime** a través del crate
`ort =2.0.0-rc.13` en modo **`load-dynamic`** (`crates/avi-stt/Cargo.toml`): no
enlaza nada en build-time ni requiere una `.lib`, y **el build local/CI de la app
no necesita compilar ni tener presente ONNX Runtime**. Por eso las cuatro
arquitecturas compilan `--features full` sin ningún paso previo de ONNX Runtime.

En runtime, `ort` carga la librería dinámica del SO. Si `ORT_DYLIB_PATH` no está
definido, usa el nombre por defecto (`onnxruntime.dll` / `libonnxruntime.so` /
`libonnxruntime.dylib`) y lo resuelve **contra el directorio del ejecutable**
(`current_exe().parent()`). Colocar la librería junto al binario basta: no hay
`rpath` ni variables de entorno que fijar.

#### Empaquetado en los artefactos de release

El binario distribuido es **autocontenido**: el usuario solo ejecuta el
instalador de la app, sin instalar nada previo. Cada job de build de CircleCI,
antes del staging del artefacto, descarga el asset oficial de Microsoft de
**ONNX Runtime 1.28.0** (pareja de `ort` rc.13) para su plataforma y coloca la
librería dinámica junto al binario con el nombre por defecto que busca `ort`:

| Job | Asset MS v1.28.0 | Librería empaquetada |
|---|---|---|
| build-windows-x64 | `onnxruntime-win-x64-1.28.0.zip` | `onnxruntime.dll` |
| build-linux-x64 | `onnxruntime-linux-x64-1.28.0.tgz` | `libonnxruntime.so` |
| build-linux-arm64 | `onnxruntime-linux-aarch64-1.28.0.tgz` | `libonnxruntime.so` |
| build-darwin-arm64 | `onnxruntime-osx-arm64-1.28.0.tgz` | `libonnxruntime.dylib` |

En **Windows**, la `onnxruntime.dll` de Microsoft es `/MD` y depende del runtime
de VC++ (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`). Para no exigir
al usuario instalar el *Visual C++ Redistributable*, esas tres DLLs se empaquetan
también junto al binario (*app-local deployment*, permitido por la licencia de
redistribución de MS); el Universal CRT (`ucrtbase.dll`) ya forma parte de
Windows 10/11. En Linux/macOS la librería depende del runtime C++ del SO
(`libstdc++` / `libc++`), ya considerado base.

Los instaladores extraen el archivo completo (layout plano) al directorio de
instalación y ejecutan el binario desde ahí, de modo que la librería llega junto
al binario y `ort` la encuentra sin cambios en los instaladores. La versión
1.28.0 no se cambia sin revalidar el binding `ort` ↔ librería nativa.

### Verificación post-build

El **smoke test del binario está automatizado en CI**: cada uno de los
4 jobs de build ejecuta `ai-voice-interconnector version` (exit 0) **y
`ai-voice-interconnector voice list`** antes de publicar el artefacto, de modo
que la ausencia de la voz `default` hace fallar el job. `version` y `voice list`
**no cargan el modelo** (las voces de fábrica viajan embebidas en el binario
vía `crates/avi-store/assets/`), así que el chequeo es de segundos.

Queda **manual** (requiere modelo, audio real y hardware por SO): `doctor`,
`setup` y una síntesis real (`speech synthesize`). La validación end-to-end de
los instaladores por SO es por diseño **externa al pipeline** (ver
`docs/GOAL.md` §Validación E2E).

### Matriz de integración con el SO

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| PATH | El one-liner `install-windows.ps1` registra `%LOCALAPPDATA%\Programs\ai-voice-interconnector` en HKCU (sin UAC); el binario `uninstall` lo revierte | `install-linux.sh` crea symlink `~/.local/bin/ai-voice-interconnector → ~/.local/opt/ai-voice-interconnector/ai-voice-interconnector`; `uninstall` lo borra | One-liner `install-macos.sh` análogo a Linux (`~/.local/bin`); Cask `brew install --cask` enlaza en `/opt/homebrew/bin` |
| Guía hacia `setup` | El one-liner encadena `setup` tras instalar | Ídem | Ídem (Cask no encadena; caveat remite a `setup`) |
| Desinstalación | `ai-voice-interconnector uninstall --force` (HKCU + dir + cleanup) o manual | `ai-voice-interconnector uninstall --force` (symlink + dir + cleanup) | `ai-voice-interconnector uninstall --force` o `brew uninstall --cask --zap` |
| Datos provisionados | `ai-voice-interconnector cleanup --voices` / `--synthetic-speech` / `--model` / `--all` (unión sin binario/PATH; sin flags → exit 2) | Ídem | Ídem |

### Limitación conocida: firma de código y notarización

Los artefactos **no están firmados ni notarizados**: en macOS, Gatekeeper
bloquea la primera apertura del binario descargado por navegador; en Windows,
SmartScreen muestra advertencia de editor desconocido si el `.zip` se baja por
navegador. El mecanismo (Mark-of-the-Web) y la firma como arreglo de fondo
están en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar). Firmar requiere
certificados de pago (Apple Developer ID, Authenticode vía SignPath OSS) y queda
registrado como goal a largo plazo en [docs/GOAL.md](GOAL.md#goal-a-largo-plazo).

Como mitigación, los **one-liners descargan por CLI** (`curl`/`Invoke-WebRequest`),
que no aplica Mark-of-the-Web, así que el archivo extraído no dispara
SmartScreen/Gatekeeper. Ver [docs/DISTRIBUTION.md](DISTRIBUTION.md) y
[docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

---

## 4. CI/CD con CircleCI

La CI es **un solo pipeline de release** (`build-all`), disparado **solo en
tags `v*`** (todos sus jobs con `filters.tags: /^v.*/` y `branches: ignore /.*/`).
La **triple puerta simétrica** `test-linux` + `test-windows` + `test-macos`
(cada uno `cargo test --all` en su SO nativo) es `requires:` de los 4 builds, de
modo que la publicación queda **mecánicamente condicionada** al resultado
completo de la suite sobre el commit taggeado, dentro de la **misma** pipeline.

- **Por qué un pipeline y no dos.** La publicación (`publish-release`) es
  irreversible, así que el release debe estar condicionado a los tests por una
  dependencia real, no por confianza. CircleCI **no encadena pipelines
  distintas**: un job solo puede depender de otro dentro del mismo workflow. La
  única forma de que los tests **bloqueen** el release es tenerlos en el grafo
  de `requires:` de los builds — es decir, en la misma pipeline. Por eso la
  triple puerta vive aquí y no en un workflow de rama separado.

- **Por qué se descartó el pipeline de rama `validate`.** Como el corte empuja
  commit+tag juntos (`git push origin main --tags`), un workflow `validate` en
  `main` disparaba una **segunda** pipeline que re-ejecutaba la **misma** triple
  suite sobre el **mismo** commit, en paralelo con `build-all`. Eso no adelantaba
  la señal (corría en paralelo, no antes) ni aportaba cobertura única: era
  duplicación pura. Al mover la triple puerta al pipeline de release, los tests
  corren **una vez** por release y bloquean de verdad la publicación.

- **Gates completos.** Además de la triple puerta: `coverage` (cargo-llvm-cov,
  **solo aquí**), los tres smoke-tests de instaladores (`test-installer-linux`
  bats, `test-installer-windows` Pester, `test-installer-macos` bats) y los gates
  `validate-licenses` (SOURCE-OFFER/THIRD-PARTY) y `validate-changelog`. Esos
  **9 gates** son `requires:` de los 4 builds nativos, que compilan las 4
  plataformas en modo release (validación de compilación por plataforma).

**Feedback pre-release.** El repo es trunk-based sobre `main` (flujo de un solo
desarrollador, sin ramas de larga vida); los commits de rama **no** disparan CI
(`build-all` es tags-only, `branches: ignore: /.*/`). La red de seguridad es el
gate al taggear: si la triple suite falla, los builds no corren y **nada se
publica** (fix-forward/revert y re-tag). No hay estado de `main` sin tag que
publique, así que no se necesita *branch protection* airtight para sostener la
garantía "commit taggeado probado".

### Simetría: 3 puertas de test vs. 4 targets de build

Las 3 puertas de test y los 4 builds (ambos en `build-all`) responden a **ejes
distintos**.

- **Por qué 3 puertas de test y 4 builds.** Los tests son **por familia de SO**:
  validan lógica Rust por SO (Windows: winsound/tray; macOS: CoreAudio; Linux:
  ALSA). Los builds son **por target de distribución**, y Linux publica **dos**
  arquitecturas. Son dos ejes ortogonales (SO × build-target).

- **Por qué el runner de `test-linux` es x86_64.** Es el executor Docker más
  barato/rápido. La suite es arch-independiente y mockea el engine, así que
  basta la arquitectura más barata.

- **Hueco de cobertura de ARM64 (divergencia aceptada).** `build-linux-arm64`
  está *gated* por tests en x86_64; el smoke test `ai-voice-interconnector
  version` (que importa el stack nativo en ARM) cubre el riesgo arch-específico.

### Arquitectura del Pipeline

**Pipeline de release `build-all`** (único; solo en tags `v*`):

```
┌────────────────────┐  ┌────────────────────┐  ┌────────────────────┐
│     test-linux     │  │    test-windows    │  │     test-macos     │
│ (cargo test --all) │  │ (cargo test --all) │  │ (cargo test --all) │
└─────────┬──────────┘  └─────────┬──────────┘  └─────────┬──────────┘
          │        (triple puerta = gate mecánico)         │
          └──────────────────────┬────────────────────────┘
        ┌───────────────┬────────┴──────┬───────────────┬──────────────┐
        │   coverage    │ validate-     │ validate-     │ test-installer-* (×3)  │
        │(cargo llvm-cov)│ licenses     │ changelog     │ (bats/Pester)          │
        └───────┬───────┴──────┬────────┴──────┬────────┴──────┬────────────────┘
                └──────────────┴───────┬───────┴───────────────┘
                          (9 gates = requires: de cada build)
         ┌───────────────┬─────────────┴─┬───────────────┐
         ▼               ▼               ▼               ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌──────────────────┐
│build-windows│ │build-linux- │ │build-linux- │ │ build-darwin-    │
│ -x64        │ │    x64      │ │   arm64     │ │     arm64        │
└─────────────┘ └─────────────┘ └─────────────┘ └──────────────────┘
     (cada build: cargo build --release --features full + smoke test version/voice list + staging tar.gz/zip)
                              │
                              ▼   publish-release → publish-metadata
```

### Jobs

| Job | Pipeline | Plataforma | Executor | Notas |
|-----|----------|------------|----------|-------|
| `test-linux` | `build-all` (gate) | Linux x64 | docker `cimg/rust:1.96.0` | `cargo test --all --verbose` |
| `test-windows` | `build-all` (gate) | Windows x64 | `win/server-2022` | `cargo test --all` en Windows nativo |
| `test-macos` | `build-all` (gate) | macOS arm64 | macos `m4pro.medium` | `cargo test --all` en macOS nativo |
| `coverage` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo llvm-cov --workspace --lcov` (solo en el tag) |
| `validate-licenses` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo run -p xtask -- source-offer --check` + `licenses --check` |
| `validate-changelog` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo run -p xtask -- changelog --check` |
| `test-installer-*` | `build-all` | por SO | bats/Pester | Smoke tests de one-liners (mock por PATH) |
| `build-windows-x64` | Windows x64 | `win/server-2022` | `cargo build --release --features full` + staging `.zip` |
| `build-linux-x64` | Linux x64 | docker `cimg/rust:1.96.0` (`large`) | `cargo build --release --features full` + staging `tar.gz` |
| `build-linux-arm64` | Linux ARM64 | docker `cimg/rust:1.96.0` (`arm.medium`) | idem, nativo aarch64 |
| `build-darwin-arm64` | macOS arm64 | macos `m4pro.medium` | idem, Xcode 26.4 |
| `publish-release` | — (CD) | docker `cimg/base:current` | Solo en tags `v*`: recolecta 4 artefactos, genera `SHA256SUMS.txt`, publica GitHub Release |
| `publish-metadata` | — (CD) | docker `cimg/base:current` | Solo en tags `v*`: renderiza Cask con `cargo xtask cask` y empuja al tap |

### Descargador nativo de modelos (`hf-hub`)

`setup` descarga los pesos de HuggingFace Hub de forma nativa vía el crate
**`hf-hub`** (rustls, sin OpenSSL: compila igual en los 4 targets) con barra de
progreso **`indicatif`**, resume por Range y validación ETag/commit-hash del
propio crate. No hay Python en la ruta de descarga.

| Modelo lógico | Repo HF | Contenido |
|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Pesos TTS (síntesis) |
| `marian-es-en` | `Helsinki-NLP/opus-mt-es-en` | Traducción es→en (CT2) |
| `marian-en-es` | `Helsinki-NLP/opus-mt-en-es` | Traducción en→es (CT2) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT Parakeet TDT v3 int8 (~600 MB, 4 artefactos: encoder-model.int8.onnx, decoder_joint-model.int8.onnx, nemo128.onnx, vocab.txt) |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base de clonado de voz (opt-in `--with-voice-cloning`) |

Los pines viven en `MODEL_REVISIONS` (`crates/avi-store/src/lib.rs`): tuplas
`(nombre_lógico, repo, revisión)`. Actualizar una revisión es una acción
deliberada y auditable.

**Ubicaciones en disco por SO.** La aplicación decide su caché (no depende del
fallback de `hf-hub`, que en Windows sin `HOME` caería en `<unidad>:\tmp`):
`hf_cache_dir()` honra `HF_HUB_CACHE` > `HF_HOME/hub` y, si no existen, fija
`{home}/.cache/huggingface/hub` — la misma convención que `huggingface_hub`
de Python en los tres SO. El cliente de descarga se construye con
`.cache_dir()` explícito, garantizando convergencia lectura=escritura.
La provisión se decide solo por presencia del snapshot HF; no hay índice
`manifest.json` intermedio (los `manifest.json` de versiones previas que
queden en disco son inertes y los barre `cleanup`):

| SO | Cache HF (`hf_cache_dir()`) | Datos del usuario (`data_dir()`) |
|----|------------------------------|----------------------------------|
| Windows | `%USERPROFILE%\.cache\huggingface\hub` | `%APPDATA%\ai-voice-interconnector\data` |
| Linux | `~/.cache/huggingface/hub` | `~/.local/share/ai-voice-interconnector/data` |
| macOS | `~/.cache/huggingface/hub` | `~/Library/Application Support/ai-voice-interconnector/data` |

`doctor` imprime la ruta resuelta (`Cache HF:` / campo `hf_cache` en `--json`)
para auditoría. `cleanup --model/--voices/--synthetic-speech/--all` borra selectivamente snapshots HF + datos de usuario (sin binario ni PATH; `cleanup` sin flags → exit 2 `usage_error`; (del binario principal); `uninstall` es el único que añade binario+PATH.

### Cacheo de dependencias y toolchain

Modelo **homogéneo** de `target-v2` para los jobs de test/cov/build vigente desde
la remediación de latencia de CI posterior a 0.20.2 (ver la sección de
`commands`/`jobs` de `.circleci/config.yml`): `test-windows` (`os: windows`,
`variant: test`), `test-linux` (`os: linux`, `variant: test`), `test-macos`
(`os: macos`, `variant: test`), `coverage` (`os: linux`, `variant: cov`) y los 4
`build-*` (`os: windows/linux/macos`, `variant: full`) usan `cargo_restore_caches`
→ restauran **registry + target-v2** (`cargo-v2-{{ arch }}-<< pipeline.parameters.rust_version >>-{{ checksum "Cargo.lock.cachekey" }}` y `target-v2-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>-<< parameters.variant >>-{{ checksum "Cargo.lock.cachekey" }}`); los jobs pequeños **xtask/changelog-gate** (`validate-licenses`, `validate-changelog`, `publish-metadata`) usan `cargo_restore_registry` → restauran **solo registry** (`cargo-v2-...`) sin `target/`, donde `sccache` basta por el tamaño mínimo de esos jobs. `test-windows`/`test-linux`/`test-macos` (`variant: test`), `coverage` (`variant: cov`) y los 4 `build-*` (`variant: full`) además restauran **y guardan** `sccache` autoconsistente por variante (`sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>-<< parameters.variant >>-{{ epoch }}`, rolling con `when: always`): cada job solo restaura el blob de su propia variante, así que sus hits no dependen de otros perfiles. Los jobs pequeños **xtask/changelog-gate** restauran `sccache` sin guardarlo (excepción restore-only documentada: compilan solo `xtask`, impacto marginal). La caché de `toolchain` (`toolchain-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>`) la restauran/guardan `test-windows`, `test-macos`, `coverage`, `build-windows-x64` y `build-darwin-arm64`; los jobs sobre Docker-Linux (`test-linux`, `build-linux-*`, jobs pequeños) no la usan por diseño (la imagen ya trae Rust preinstalado). La clave de `registry`/`target-v2` NO se deriva de `Cargo.lock` directo, sino de un **lock normalizado** (`Cargo.lock.cachekey`) que el primer step de `cargo_restore_caches` genera con un transform de texto (`perl -0777`, ejecutado en `shell: bash`): neutraliza la línea `version` del propio crate (`ai-voice-interconnector` → `0.0.0`) dejando el resto del lock intacto. Motivo: cada release bumpea esa versión, así que el checksum de `Cargo.lock` cambiaría en cada corte aunque las dependencias no varíen, invalidando la clave exacta y dejando todo al frágil fallback por prefijo. Con la normalización la clave exacta es estable entre releases y solo cambia ante cambios reales de dependencias. Generar la clave con un transform de texto (en vez de compilar `xtask` desde cero antes de restaurar caché) evita compilar el árbol de dependencias de cargo en cada job —incluido `windows-sys`/`dlltool`, ausente en `test-windows` antes de instalar MSYS2— y su correspondiente coste de red en frío.

`sccache_save_cache` guarda `~/.cache/sccache` de forma **incondicional** con clave *rolling* por `{{ epoch }}` (`when: always`): cada corrida escribe una entrada nueva y la restauración toma la más reciente por prefijo. No existe un guardado condicional por hit-rate, y a propósito: un umbral de ese tipo es incoherente en este pipeline. Todo hit &lt; 100 % implica *misses* que sccache **acaba de compilar y escribir** en el store, así que omitir el guardado descartaría esos objetos nuevos (la siguiente corrida los volvería a fallar); y vaciar el directorio para abaratar la subida envenenaría la caché restaurada. Como cada `build-*` bumpea la versión del crate —recompilándolo siempre— el store cambia en cada tag, de modo que cualquier dedup por contenido casi nunca se dispararía: el ahorro real se limita a re-ejecutar el mismo tag sin cambios, marginal frente al riesgo de tocar la ruta crítica del release. Cada job pesado combina `target-v2` con `sccache` autoconsistente por variante (`test-windows`/`test-linux`/`test-macos` con `variant: test`, `coverage` con `variant: cov`, `build-*` con `variant: full`); `sccache` computa sus hits por hash de contenido, así que es inmune al `mtime` y cubre la recompilación residual que `target-v2` no evita. Dos causas explican el coste previo (evidencia v0.20.4): (1) la clave de `sccache` no llevaba `variant`, de modo que `test-windows` y `coverage` restauraban un blob de perfil `full` ajeno —tasas medidas vía `sccache --show-stats` de **0.00 %** en ambos— y además nunca guardaban el suyo (0 % perpetuo, frente al 96.43 % de `build-windows-x64`, que sí guarda y consume su propio perfil); las tasas históricas de Linux 3.21 % / macOS 3.04 % se midieron bajo esa misma colisión de perfiles y quedan como dato histórico, no vigente. (2) `target-v2` se invalida por `mtime`: parcial en los tres SO (los crates propios del workspace, dependencias de ruta local, recompilan siempre) y total solo en Windows (0 `Fresh` / 154 `Compiling`, incluyendo dependencias externas con checksum estable que en Linux/macOS sobreviven: 264/290 y 287/313 `Fresh`).

Matriz de invalidación por familia de caché:

| Caché | Namespace | Se invalida cuando… |
|-------|-----------|---------------------|
| `registry` (`cargo-v2`) | `v2` | cambian dependencias reales en `Cargo.lock` (no el bump de versión del crate) |
| `target-v2` (`test-windows`/`test-linux`/`test-macos`/`coverage` `test`/`test`/`test`/`cov` + `build-*` `full`, `os: windows/linux/macos`) | `v2` | idem `registry`, variant-específico (`test`/`cov`/`full`) + `os`. Los crates propios del workspace (path dependencies) recompilan siempre por fingerprinting por `mtime` (parcial en los tres SO; total solo en Windows, que pierde también dependencias externas). `build-*` restaura `target-v2-full`; el crate raíz (`ai-voice-interconnector`) recompila igualmente en cada release porque el bump de `VERSION` cambia el `mtime` y la `-C metadata` del propio fingerprint, ver determinismo debajo. |
| `toolchain` (`toolchain-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>`) | `v1` | cambia `rust_version` |
| `msys2` | `v1` | cambian los pines de versión MSYS2 (release base + gcc/openblas/make) |
| `sccache` (`sccache-v1-{{ arch }}-<< parameters.os >>-<< pipeline.parameters.rust_version >>-<< parameters.variant >>-{{ epoch }}`, rolling) | `v1` | rolling por prefijo de la misma `variant` (`test`/`cov`/`full`) en el mismo `arch`+`os`+toolchain; save **incondicional** (`when: always`) en cada job pesado (los jobs xtask pequeños son restore-only), la restauración toma la entrada más reciente del prefijo. Sin `variant` en la clave, un job restauraba el blob de otro perfil y jamás obtenía hits (causa del 0.00 % de `test-windows`/`coverage` en v0.20.4) |
| `ort-bundle` | `v1` | cambia `ort_version` (`1.28.0`) — sin `Cargo.toml` en la clave para no invalidar por bump del crate |
| `tts` (`qwen_tts.exe`) | `v1` | cambia `vendor/qwen3-tts/.engine-cachekey` (agregado de `Makefile` + `*.c/*.h` + `third_party/ingot`) + `msys2_gcc_version`/`openblas` |

> **Determinismo de releases (binary stale) — heterogéneo con `target` en `build-*`:** `build-*` restaura `target-v2-full` (`cargo_restore_caches` con `variant: full`) y compila directo, sin ningún paso de limpieza manual: el bump de `VERSION` en el crate raíz basta por sí solo para invalidar su fingerprint de cargo (el checkout deja `src/main.rs`/`Cargo.toml` con `mtime` posterior al de cualquier artefacto restaurado, y el cambio de versión del paquete altera la `-C metadata` que cargo usa para nombrar la unidad de compilación), así que el binario siempre se recompila con la versión correcta aunque `target/` venga poblado de un release anterior. `sccache` aporta Rust por hash de contenido, `target` aporta `OUT_DIR` C++ (`ct2rs`/`oneDNN`/`sentencepiece`) que `sccache` no cubre; además `sccache` nunca cachea crates `--crate-type bin` (el binario final, los build scripts y `xtask` siempre pasan por `rustc` real, nunca por un hit de sccache), por lo que un "binario stale servido por sccache" es imposible por construcción. El `perl` del paso «Generar Cargo.lock.cachekey» normalizado mantiene la clave estable y el smoke-test `version --json == CIRCLE_TAG` valida fail-fast por si algún otro mecanismo dejara el binario desalineado. Medición `v0.18.23` con opción C pura: wall `50m50s`, `build-windows 40m31s` (`37m21s` `cargo build`, `87%` Rust hit pero `cmake` C++ recompilado). Medición `v0.20.5` con `sccache` por variante: wall `~25m` (`build-windows ~12m` en primer ciclo con familia `full` fría). Medición `v0.20.6`: wall `~23m` con las familias ya pobladas (`test-windows` 86,33 % de hits, `coverage` 76,92 %, `build-windows` 74,07 % — primer release con hits donde había 0,00 %).

Ver `.circleci/config.yml` para claves exactas. `coverage` genera `lcov.info` en una sola corrida y el resumen con `cargo llvm-cov report` (sin re-ejecutar la suite).

### Reproducibilidad: pines por digest y sus implicaciones

Las imágenes `cimg/rust:1.96.0` van pineadas por digest (`@sha256:...`), y
`rust_version` + `ort_version` son parámetros únicos del pipeline (`pipeline.parameters`).
Ver `.circleci/config.yml` §Reproducibilidad para procedimiento de bump (bumpear
`ort_version` invalida `ort-v1` en el siguiente tag).

El archivo de configuración completo está en `.circleci/config.yml`.

### CD: publicación del GitHub Release (`publish-release`)

Al pushear un tag `v*`, además de tests + builds corre `publish-release`
(estrategia GitHub Releases). Recolecta los 4 artefactos **versionados** por
`persist_to_workspace`/`attach_workspace`, genera `SHA256SUMS.txt`, extrae las
notas de la sección `[X.Y.Z]` de `CHANGELOG.md` (fail-fast si no existe) y
publica el GitHub Release directo (sin borrador).

### Guardarraíles durables de la suite de tests (`tests/cli_golden.rs`)

Dos invariantes de la suite dorada son fáciles de "optimizar" por error y
quedan documentados aquí de forma autocontenida:

- **Serialización de residentes TTS (`TTS_LOCK`/`lock_tts()`).** Cada
  residente TTS activo ocupa un pico medido de ~2.7 GB de RAM. La suite
  serializa deliberadamente sus tests pesados de inferencia con un mutex
  global (`TTS_LOCK`, junto a `lock_tts()` en `tests/cli_golden.rs`) para
  garantizar que nunca haya más de un residente TTS vivo a la vez. Quitar o
  debilitar ese lock para paralelizar los tests pesados arriesga un OOM del
  runner de CI, no solo una degradación de tiempo: el lock no se elimina.
- **El warning de libtest ">60 seconds" no es un cuelgue.** Con la
  serialización anterior, es esperable que `cargo test` reporte que un test
  de la serie pesada "has been running for over 60 seconds" mientras espera
  turno para adquirir `TTS_LOCK`. Es espera de mutex esperada, no trabajo
  atascado: no se le añaden reintentos ni timeouts para silenciarlo.

---

## 5. Distribución de artefactos

El **deliverable** que se publica a usuarios es el **archivo comprimido** de
cada target (binario Rust + los 4 documentos de licencia + `ort-bundle` +
`qwen_tts` vendido), con su nombre de
release **versionado y con arch** (p. ej.
`ai-voice-interconnector-<ver>-x86_64-windows.zip`). Estos cuatro archivos llegan
al GitHub Release a través de `persist_to_workspace` / `attach_workspace`.

El output del empaquetado en el runner vive en `artifacts/`:

```
artifacts/
├── ai-voice-interconnector-<ver>-x86_64-windows.zip   # Windows x64 (.zip)
├── ai-voice-interconnector-<ver>-x86_64-linux.tar.gz  # Linux x64
├── ai-voice-interconnector-<ver>-arm64-linux.tar.gz   # Linux ARM64
└── ai-voice-interconnector-<ver>-arm64-macos.tar.gz   # macOS (Apple Silicon)
```

`publish-release` recoge estos cuatro archivos por `attach_workspace`, calcula
`SHA256SUMS.txt` sobre ellos y crea el GitHub Release. Cada archivo tiene layout
plano (binario + 4 documentos + `ort-bundle` + `qwen_tts` vendido, en la raíz).

---

## 9. Build nativo del motor TTS (Rust/qwen_tts)

> Esta sección documenta el toolchain C del motor Qwen3-TTS vigente.

### Interfaz uniforme: `xtask build-engine`

El motor se **compila desde fuente en CI en las 4 plataformas** (antes Windows
arrastraba un blob `qwen_tts.exe` de 33 MB versionado a mano; **ya no se
versiona**). Los 4 jobs de build invocan una única interfaz:

```bash
cargo run -p xtask -- build-engine --self-test
```

`build-engine` (`crates/xtask/src/main.rs`) oculta el mecanismo por plataforma:
en Unix invoca `make` con el entorno heredado; en Windows invoca `mingw32-make`
con el entorno MSYS2 UCRT64 augmentado (`PATH` con `ucrt64\bin` + `usr\bin` para
`cygpath`, `MSYSTEM=UCRT64`), leyendo la raíz de `MSYS2_ROOT` (default
`C:\msys64`). Pasa `SIMD=auto` por defecto (la política SIMD es autoridad única
del `Makefile`), compila con `make blas` y verifica con `--self-test` (oráculo de
kernels, sin pesos del modelo). El mismo comando sirve para CI, dev local y este
doc. En dev local sin MSYS2, Windows falla con un mensaje que guía a instalar
UCRT64 o definir `MSYS2_ROOT`.

**Toolchain vigente (Windows):** MSYS2 UCRT64 **gcc 16.2.0** (Rev3),
`mingw-w64-ucrt-x86_64-openblas 0.3.34-1`, `mingw32-make 4.4.1-5`
(`vendor/qwen3-tts/Makefile:3-5,17-85`). En CI se **aprovisiona pineado**: el job
`build-windows-x64` extrae el release base de MSYS2 (`msys2_base_release`,
`2026-06-11`) y sincroniza las versiones pineadas (parámetros `msys2_*_version`
del pipeline, espejo de esta tabla), cacheando `C:\msys64` por clave de versión
(determinismo de release). El log del bootstrap captura `pacman -Q` + `gcc
--version` como evidencia de la **GCC Runtime Library Exception** (libgfortran/
libquadmath/libgcc estáticos en el `.exe`). Si la versión instalada difiere del
pin, el bootstrap registra `[WARN]` y continúa usando la instalada como evidencia
(`log on drift` desde `v0.17.1`, no `fail-fast`). WSL oráculo: Ubuntu gcc
**15.2.0-16ubuntu1**, `libopenblas-dev 0.3.32+ds-5`.

| Plataforma | `ARCH_FLAGS` | `CFLAGS_BASE` | `LDLIBS` / BLAS | Shims |
|---|---|---|---|---|
| Windows UCRT64 | `-mavx2 -mfma` (Haswell 2013+, `SIMD=auto`; `SIMD=scalar` vacía) | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `-static -L$(UCRT64_LIB) -lopenblas -lgomp -lws2_32 -lwinpthread -lm` (33 MB autocontenido) | `third_party/ingot/mingw_shim/unistd.h`, `sys/mman.h` vía `-include` |
| Linux x86_64 | `-mavx2 -mfma` | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `-lopenblas` | — |
| Linux ARM64 | `-march=armv8-a` (portable; NEON baseline ARMv8-A. `SIMD=native` → `-march=native`) | idem | `-lopenblas` | — |
| macOS (arm64) | `-march=native` (single-vendor) | idem | `-framework Accelerate` | — |

> **Baseline ARM portable.** La rama ARM del `Makefile` honra `SIMD` como x86:
> por defecto `-march=armv8-a` (NEON garantizado en todo ARMv8-A), no
> `-march=native` — que acoplaría el codegen a la microarquitectura del runner y
> haría `SIGILL` en CPUs de campo más viejas. `SIMD=native` recupera `-march=native`
> para el dev box (misma CPU build=run). macOS conserva `-march=native` (host
> single-vendor).

En CI Windows el binario del motor (`vendor/qwen3-tts/qwen_tts.exe`) se cachea con
clave `tts-v1-{{ arch }}-gcc<< pipeline.parameters.msys2_gcc_version >>-ob<< pipeline.parameters.msys2_openblas_version >>-{{ checksum "vendor/qwen3-tts/.engine-cachekey" }}` donde
`.engine-cachekey` es el agregado determinista de `Makefile` + `*.c/*.h` (incluye `vendor/lz4.*`) + `third_party/ingot/**/*.{c,h}`.
Si existe tras `restore_cache` (clave exacta sin fallback) solo se verifica con `--self-test`;
si no, se compila. El bundle ONNX Runtime (`ort-bundle/`) se cachea con
`ort-v1-win-x64-<< pipeline.parameters.ort_version >>` y guarda tras el bundling.

Ver `vendor/qwen3-tts/CLAUDE.md` y `crates/avi-tts/src/lib.rs` para el contrato
de invocación (`--int4 -j 4 --stream`, `GenerationOptions::produccion()` temp
0.35 seed 4).

