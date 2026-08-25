#!/bin/sh
# Instalador auto-hospedado de ai-voice-interconnector para macOS (Apple Silicon).
#
# Uso:
#   curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-macos.sh | sh
#
# Resuelve el último Release de GitHub, descarga el tar.gz de arm64 y
# SHA256SUMS.txt, verifica el checksum (abortando si no coincide), lo extrae en
# ~/.local/opt/ai-voice-interconnector/, limpia la cuarentena de Gatekeeper
# (legítimo: el usuario ya expresó intención ejecutando este script), crea el
# symlink de PATH en ~/.local/bin y encadena `setup` (que ofrece descargar el
# modelo de voz). Ver docs/SELF-HOSTED-INSTALL.md para el diseño completo.
#
# Espejo estructural de install-linux.sh (Linux). Sin `sudo`: instalación per-user.
# Solo asume binarios del sistema base de macOS (no `sha256sum` — se usa
# `shasum`; no `jq` — parseo con grep/sed).
#
# POSIX sh: sin bashismos.

set -eu

REPO="CristianRojas-SoftwareEngineer/AI-Voice-InterConnector"
INSTALL_DIR="${HOME}/.local/opt/ai-voice-interconnector"
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
require_cmd tar
require_cmd xattr
require_cmd mkdir

# --- Guard de arquitectura -------------------------------------------------
# ai-voice-interconnector publica solo el tar.gz de Apple Silicon (arm64). Mac
# Intel no está soportado (limitación de toolchain documentada en el README).
machine="$(uname -m)"
if [ "$machine" != "arm64" ]; then
    log "Alternativa para Mac Intel: compila desde la fuente (docs/BUILD.md)."
    fail "arquitectura no soportada: $machine (ai-voice-interconnector solo publica tar.gz para Apple Silicon / arm64 en macOS)"
fi
log "Arquitectura detectada: $machine"

# --- Resolver el release y elegir los assets ------------------------------
log "Resolviendo el último release de $REPO..."
release_json="$(curl -fsSL "$API_URL")" || fail "no se pudo consultar $API_URL"

# Extrae las URLs de descarga sin depender de jq (parseo con grep/sed, como
# install-linux.sh): el tar.gz de arm64 de macOS y SHA256SUMS.txt.
archive_url="$(printf '%s' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*-arm64-macos\.tar\.gz"' \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"
sums_url="$(printf '%s' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*SHA256SUMS\.txt"' \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"

[ -n "$archive_url" ] || fail "no se encontró un tar.gz de arm64 para macOS en el último release"
[ -n "$sums_url" ] || fail "no se encontró SHA256SUMS.txt en el último release"

archive_name="$(basename "$archive_url")"
log "Asset seleccionado: $archive_name"

# --- Descarga y verificación de checksum ----------------------------------
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

log "Descargando $archive_name..."
curl -fsSL -o "$work_dir/$archive_name" "$archive_url" || fail "descarga del archivo fallida"

log "Descargando SHA256SUMS.txt..."
curl -fsSL -o "$work_dir/SHA256SUMS.txt" "$sums_url" || fail "descarga de SHA256SUMS.txt fallida"

log "Verificando checksum..."
( cd "$work_dir" && grep "$archive_name\$" SHA256SUMS.txt | shasum -a 256 -c - ) \
    || fail "el checksum de $archive_name no coincide con SHA256SUMS.txt; instalación abortada"

# --- Instalación -----------------------------------------------------------
# El directorio de instalación es propiedad exclusiva del proyecto: se limpia
# por completo antes de extraer para no dejar archivos huérfanos de una versión
# anterior (el archivo trae layout plano: binario + documentos de licencia).
rm -rf "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR"
tar -xzf "$work_dir/$archive_name" -C "$INSTALL_DIR" || fail "no se pudo extraer $archive_name"

target="$INSTALL_DIR/ai-voice-interconnector"
[ -x "$target" ] || fail "el binario esperado no existe o no es ejecutable: $target"
chmod +x "$target"
log "Instalado en: $INSTALL_DIR"

# --- Limpieza de cuarentena (Gatekeeper) ----------------------------------
# El usuario ya expresó intención ejecutando este script; limpiar el atributo
# com.apple.quarantine evita la advertencia de reputación en el primer arranque.
log "Limpiando la cuarentena de Gatekeeper..."
xattr -dr com.apple.quarantine "$target" 2>/dev/null || true

# --- Integración de PATH per-user -----------------------------------------
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
