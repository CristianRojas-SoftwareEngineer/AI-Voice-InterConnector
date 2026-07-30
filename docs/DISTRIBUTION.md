# Canales de distribución

`tts-sidecar` se distribuye por **dos canales independientes**, publicados
simultáneamente en cada tag `v*`: el canal **nativo** (binarios PyInstaller
por SO, ver [docs/BUILD.md](BUILD.md)) y el canal **PyPI** (`pip`/`uv
tool`/`pipx`). Ambos instalan el mismo CLI con el mismo contrato programático
(exit codes, esquemas `--json`); difieren en audiencia, prerequisitos y
fricción del primer arranque.

## Tabla de contenidos

- [Los dos canales](#los-dos-canales)
- [Instalación por canal](#instalación-por-canal)
- [Por qué el canal pip evita SmartScreen/Gatekeeper](#por-qué-el-canal-pip-evita-smartscreengatekeeper)
- [Registro de la decisión A vs. B](#registro-de-la-decisión-a-vs-b)
- [Flujo de publicación (CI)](#flujo-de-publicación-ci)

## Los dos canales

| | Canal nativo | Canal PyPI |
|---|---|---|
| **Audiencia** | Usuario final sin Python instalado | Usuario técnico con Python 3.13+ |
| **Instalación** | Instalador `.exe`/AppImage/`.dmg` por SO | `uv tool install tts-sidecar` / `pipx install tts-sidecar` |
| **Tamaño** | ~1-2 GB (bundle onedir autocontenido) | Descarga las dependencias desde PyPI (torch, etc.) al instalar |
| **Dependencias del sistema** | Ninguna (autocontenido) | Linux: requiere `libportaudio2` del sistema (`sounddevice` no trae wheels con PortAudio embebido) |
| **SmartScreen / Gatekeeper** | Bloquea el primer arranque (ver más abajo) | No aplica: el launcher lo genera `pip`/`uv` localmente, sin Mark-of-the-Web |
| **Actualización** | Re-ejecutar el one-liner por SO (Linux/macOS `curl \| sh`, Windows `irm \| iex`; limpia la versión anterior) o `brew upgrade --cask` (ver `USAGE.md`) | `uv tool upgrade tts-sidecar` / `pipx upgrade tts-sidecar` |
| **Desinstalación** | Linux: `setup --uninstall` (un paso); macOS: `.command` sin `sudo` + Papelera o `brew uninstall --cask --zap`; Windows: desinstalador Inno Setup — todos + `cleanup --all` (ver `USAGE.md`) | `uv tool uninstall tts-sidecar` / `pipx uninstall tts-sidecar` |
| **Publicación en CI** | Job `publish-release` → GitHub Release directo | Job `publish-pypi` → publicación directa a PyPI |
| **Reversibilidad de la publicación** | El Release es público al publicarse: revertir implica borrar un Release ya público | Irreversible: un paquete subido no se puede sobrescribir, solo yankear |

En ambos casos, `setup` provisiona el modelo `es-mx-latam` en la caché de
HuggingFace del usuario (`~/.cache/huggingface/hub`) de forma idéntica: el
modelo nunca viaja dentro del paquete, en ningún canal.

## Instalación por canal

### Canal nativo (binarios por SO)

Ver [README.md](../README.md#instalación) y [USAGE.md](../USAGE.md#instalación)
para el detalle completo por SO (instalador de Windows, AppImage de Linux,
`.dmg` de macOS).

Las tres plataformas tienen además una **instalación auto-hospedada de
una línea** (`curl | sh` / `irm | iex`), capa adicional sobre el mismo
artefacto nativo (no un canal nuevo), con un script por SO:

- **Linux** — `install-linux.sh` (`curl | sh`) automatiza la descarga, verificación
  de checksum e instalación del `.AppImage` (eliminando la versión anterior al
  actualizar) y encadena `setup`.
- **macOS** — `install-macos.sh` (`curl | sh`) descarga el `.dmg` de arm64,
  verifica el checksum con `shasum`, copia el `.app` a `~/Applications` sin
  `sudo`, limpia la cuarentena de Gatekeeper, crea el symlink per-user en
  `~/.local/bin` y encadena `setup`. **Vía complementaria** para usuarios de
  Homebrew: el Cask del tap propio (`brew tap
  CristianRojas-SoftwareEngineer/tts-sidecar && brew install --cask
  tts-sidecar`), que resuelve PATH, desinstalación (`--zap`) y cuarentena sin
  intervención manual, pero exige Homebrew y no provisiona el modelo.
- **Windows** — `install-windows.ps1` (`irm | iex`) descarga el instalador Inno Setup
  del release, verifica su checksum y lo ejecuta en silencio (instalación
  per-user: `%LOCALAPPDATA%\Programs`, PATH de usuario en HKCU, sin UAC),
  terminando con `tts-sidecar setup`.

El `.dmg` descargado a mano (con sus scripts `.command` de
instalación/desinstalación, ahora per-user sin `sudo`) sigue siendo un canal
válido en paralelo. La justificación técnica de la vía de Windows es el *Mark of
the Web* (MOTW): el navegador sí sella el `.exe` con la marca de Internet
(`ZoneId=3`) y dispara SmartScreen, pero la descarga por CLI (`curl`, `gh`,
PowerShell `Invoke-WebRequest`/`WebClient`) **no** la aplica, así que el
instalador bajado por `install-windows.ps1` no dispara SmartScreen al ejecutarse. El
binario sigue sin firmar: Microsoft Defender Antivirus es independiente del MOTW
y puede marcarlo (ver runbook WDSI en `SECURITY.md`); la advertencia de
SmartScreen para la descarga por navegador solo la resuelve la firma de código
Authenticode. Diseño completo de los tres instaladores en
[docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

### Canal PyPI (`uv tool install` / `pipx`)

```bash
# Con uv (recomendado: https://docs.astral.sh/uv/)
uv tool install tts-sidecar
tts-sidecar setup      # provisiona el modelo, idéntico al canal nativo
tts-sidecar speech say --text "Hola mundo"

# Con pipx
pipx install tts-sidecar
tts-sidecar setup
```

**Prerequisito en Linux**: `sounddevice` requiere la librería del sistema
`libportaudio2` para reproducir audio (no la trae empaquetada, a diferencia
del bundle nativo). Instálala antes de `setup` si vas a reproducir audio
directamente (no es necesaria si solo usas `speech synthesize --text T --label L` a archivo):

```bash
# Debian/Ubuntu
sudo apt install libportaudio2

# Fedora
sudo dnf install portaudio
```

Windows y macOS no requieren ningún paquete adicional del sistema (`pycaw` y
`afplay` no dependen de PortAudio).

**Actualización**:

```bash
uv tool upgrade tts-sidecar
# o
pipx upgrade tts-sidecar
```

**Desinstalación**:

```bash
tts-sidecar cleanup --all --yes   # elimina modelo y voces de usuario (igual que en el canal nativo)
uv tool uninstall tts-sidecar
# o
pipx uninstall tts-sidecar
```

## Por qué el canal pip evita SmartScreen/Gatekeeper

Windows SmartScreen y macOS Gatekeeper inspeccionan archivos descargados de
internet que llevan la **Mark of the Web** (Windows) o el atributo de
cuarentena `com.apple.quarantine` (macOS): ambos los añade el navegador al
descargar un instalador. El canal pip no distribuye un binario descargado:
`pip`/`uv`/`pipx` descargan el **paquete** (wheel) desde PyPI y generan el
**launcher ejecutable localmente**, en la máquina del usuario, en el momento
de la instalación. Un archivo generado localmente no lleva Mark-of-the-Web ni
cuarentena, así que ninguno de los dos sistemas de reputación se activa. Esta
es una propiedad estructural del mecanismo de instalación, no una mitigación
parcial: el canal pip no tiene el problema que el canal nativo sí tiene (ver
`SECURITY.md` §"Artefactos sin firmar" y `docs/BUILD.md` §"Limitación
conocida: firma de código y notarización" para el detalle del canal nativo).

## Registro de la decisión A vs. B

**Contexto**: tras el release `v0.1.1`, los binarios del canal nativo seguían
sin firma de código, disparando SmartScreen/Gatekeeper en cada primer
arranque. Existían dos estrategias no excluyentes para reducir esa fricción:
**A** (añadir un segundo canal de distribución sin el problema de raíz) y
**B** (firmar/notarizar los binarios del canal nativo existente vía SignPath
Foundation + Apple Developer ID).

**Alternativas consideradas**:
- Solo B: elimina la advertencia en el canal nativo, pero requiere aprobación
  externa de un programa de terceros (SignPath OSS) y una cuenta Apple
  Developer de pago; sin fecha de cierre garantizada.
- Solo A: no resuelve la fricción del canal nativo (sigue siendo el canal
  recomendado para usuarios no técnicos), pero es implementable de inmediato
  con herramientas ya disponibles (PyPI, `uv`).
- A y B en paralelo (decisión tomada): A se implementa ahora como canal
  alternativo inmediato para la audiencia técnica; B queda registrado como
  goal a largo plazo en [docs/GOAL.md](GOAL.md#goal-a-largo-plazo)
  para cuando se cumplan sus condiciones de entrada.

**Decisión**: implementar A en este ciclo (este documento y el job
`publish-pypi`) y registrar B como compromiso futuro, sin bloquear uno con el
otro.

**Consecuencias**: el proyecto mantiene dos canales de publicación
automática por tag, con matrices de trade-offs distintas (ver tabla arriba);
la resolución completa de la fricción de SmartScreen/Gatekeeper en el canal
nativo sigue pendiente de B.

## Flujo de publicación (CI)

En cada tag `v*`, tras la triple puerta de tests (`test-linux`,
`test-windows`, `test-macos`), el job `publish-pypi` corre **en paralelo** a
los cuatro builds nativos (no depende de ellos: el sdist/wheel no requiere
PyInstaller):

1. Construye sdist y wheel (`python -m build`).
2. Valida la metadata (`twine check`).
3. Instala el wheel en un venv limpio y verifica que `tts-sidecar version`
   coincide con el tag y que la voz `default` está presente
   (`voices.list_voices()`).
4. Publica a PyPI (`twine upload`) usando `PYPI_API_TOKEN` del context
   aislado `pypi-publish` (ningún otro job ve ese token).

**La publicación a PyPI es irreversible**: al igual que el GitHub Release —que
se publica directo y, para revertirlo, obliga a borrar un Release ya público—,
un paquete subido a PyPI no se puede sobrescribir; solo yankear una versión y
publicar una nueva. Por eso el smoke test del paso 3 corre siempre antes del
upload. Prerequisito
operativo (una sola vez): el context `pypi-publish` en CircleCI con la
variable `PYPI_API_TOKEN` (token API de PyPI con scope al proyecto), análogo
al context `github-release` existente (ver `docs/RELEASING.md`).
