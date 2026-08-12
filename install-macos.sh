#!/bin/sh
# Instalador auto-hospedado de ai-voice-interconnector para macOS (Apple Silicon).
#
# Uso:
#   curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-macos.sh | sh
#
# Resuelve el último Release de GitHub, descarga el .dmg de arm64 y
# SHA256SUMS.txt, verifica el checksum (abortando si no coincide), monta el
# .dmg sin privilegios, copia el .app a ~/Applications, limpia la cuarentena
# de Gatekeeper (legítimo: el usuario ya expresó intención ejecutando este
# script), crea el symlink de PATH en ~/.local/bin y encadena `setup` (que
# ofrece descargar el modelo de voz). Ver docs/SELF-HOSTED-INSTALL.md para el
# diseño completo.
#
# Espejo estructural de install-linux.sh (Linux). Sin `sudo`: instalación per-user.
# Solo asume binarios del sistema base de macOS (no `sha256sum` — se usa
# `shasum`; no `jq` — parseo con grep/sed).
#
# POSIX sh: sin bashismos.

set -eu

REPO="CristianRojas-SoftwareEngineer/AI-Voice-InterConnector"
APPLICATIONS_DIR="${HOME}/Applications"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"

log() {
    printf '%s\n' "$*" >&2
}

fail() {
    log "ERROR: $*"
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "falta el comando requerido: $1"
}

require_cmd curl
require_cmd uname
require_cmd shasum
require_cmd hdiutil
require_cmd xattr
require_cmd ditto
require_cmd mkdir

# --- Guard de arquitectura -------------------------------------------------
# ai-voice-interconnector publica solo el .dmg de Apple Silicon (arm64). Mac Intel no está
# soportado (limitación de toolchain documentada en el README).
machine="$(uname -m)"
if [ "$machine" != "arm64" ]; then
    log "Alternativas para Mac Intel: instala desde PyPI ('uv tool install ai-voice-interconnector' o 'pipx install ai-voice-interconnector') o compila desde la fuente (docs/BUILD.md)."
    fail "arquitectura no soportada: $machine (ai-voice-interconnector solo publica .dmg para Apple Silicon / arm64 en macOS)"
fi
log "Arquitectura detectada: $machine"

# --- Resolver el release y elegir los assets ------------------------------
log "Resolviendo el último release de $REPO..."
release_json="$(curl -fsSL "$API_URL")" || fail "no se pudo consultar $API_URL"

# Extrae las URLs de descarga sin depender de jq (parseo con grep/sed, como
# install-linux.sh): el .dmg de arm64 y SHA256SUMS.txt.
dmg_url="$(printf '%s' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*arm64\.dmg"' \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"
sums_url="$(printf '%s' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*SHA256SUMS\.txt"' \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"

[ -n "$dmg_url" ] || fail "no se encontró un .dmg de arm64 en el último release"
[ -n "$sums_url" ] || fail "no se encontró SHA256SUMS.txt en el último release"

dmg_name="$(basename "$dmg_url")"
log "Asset seleccionado: $dmg_name"

# --- Descarga y verificación de checksum ----------------------------------
work_dir="$(mktemp -d)"
mount_point="$(mktemp -d)"
# El trap desmonta el volumen (si sigue montado) y borra los temporales.
cleanup() {
    hdiutil detach "$mount_point" >/dev/null 2>&1 || true
    rm -rf "$work_dir" "$mount_point"
}
trap cleanup EXIT

log "Descargando $dmg_name..."
curl -fsSL -o "$work_dir/$dmg_name" "$dmg_url" || fail "descarga del .dmg fallida"

log "Descargando SHA256SUMS.txt..."
curl -fsSL -o "$work_dir/SHA256SUMS.txt" "$sums_url" || fail "descarga de SHA256SUMS.txt fallida"

log "Verificando checksum..."
( cd "$work_dir" && grep "$dmg_name\$" SHA256SUMS.txt | shasum -a 256 -c - ) \
    || fail "el checksum de $dmg_name no coincide con SHA256SUMS.txt; instalación abortada"

# --- Montaje, copia y desmontaje ------------------------------------------
log "Montando $dmg_name..."
hdiutil attach "$work_dir/$dmg_name" -nobrowse -readonly -mountpoint "$mount_point" >/dev/null \
    || fail "no se pudo montar el .dmg"

# Localiza el .app dentro del volumen montado (no se hardcodea el nombre).
app_src=""
for candidate in "$mount_point"/*.app; do
    [ -d "$candidate" ] || continue
    app_src="$candidate"
    break
done
[ -n "$app_src" ] || fail "no se encontró ningún .app dentro del .dmg montado"

app_name="$(basename "$app_src")"
app_dest="$APPLICATIONS_DIR/$app_name"

mkdir -p "$APPLICATIONS_DIR"
# ditto preserva metadatos y reemplaza la instalación anterior si existe.
if [ -d "$app_dest" ]; then
    log "Reemplazando la instalación anterior en $app_dest..."
    rm -rf "$app_dest"
fi
log "Copiando $app_name a $APPLICATIONS_DIR..."
ditto "$app_src" "$app_dest" || fail "no se pudo copiar el .app a $APPLICATIONS_DIR"

hdiutil detach "$mount_point" >/dev/null 2>&1 || true

# --- Limpieza de cuarentena (Gatekeeper) ----------------------------------
# El usuario ya expresó intención ejecutando este script; limpiar el atributo
# com.apple.quarantine evita la advertencia de reputación en el primer arranque.
log "Limpiando la cuarentena de Gatekeeper..."
xattr -dr com.apple.quarantine "$app_dest" 2>/dev/null || true

# --- Integración de PATH per-user -----------------------------------------
target="$app_dest/Contents/MacOS/ai-voice-interconnector"
[ -x "$target" ] || fail "el binario esperado no existe o no es ejecutable: $target"

link_dir="${HOME}/.local/bin"
link="$link_dir/ai-voice-interconnector"
mkdir -p "$link_dir"
ln -sf "$target" "$link"
log "Symlink creado: $link -> $target"

# ~/.local/bin no está en el PATH por defecto de zsh en macOS: avisa sin mutar
# los dotfiles del usuario (mismo patrón que cli.py::_integrate_linux_path).
case ":${PATH:-}:" in
    *":$link_dir:"*)
        ;;
    *)
        log ""
        log "AVISO: $link_dir no está en tu PATH."
        log "Añade esta línea a tu shell profile (~/.zshrc) y reinicia la terminal:"
        log '    export PATH="$HOME/.local/bin:$PATH"'
        ;;
esac

# --- Provisión del modelo -------------------------------------------------
log ""
log "Ejecutando 'ai-voice-interconnector setup' (ofrece descargar el modelo de voz)..."
"$target" setup

log "Instalación completa."
