#!/bin/sh
# Instalador auto-hospedado de ai-voice-interconnector para Linux.
#
# Uso:
#   curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-linux.sh | sh
#
# Resuelve el último Release de GitHub, elige el tar.gz de la arquitectura
# del host, descarga el archivo y SHA256SUMS.txt, verifica el checksum
# (abortando si no coincide), lo extrae en ~/.local/opt/ai-voice-interconnector/,
# crea el symlink de PATH en ~/.local/bin (el `setup` de Rust ya no integra el
# PATH) e invoca `setup`, que ofrece descargar el modelo de voz. Ver
# docs/SELF-HOSTED-INSTALL.md para el diseño completo.
#
# POSIX sh: sin bashismos, para funcionar bajo `sh` en cualquier distro (dash,
# busybox sh, bash en modo POSIX).

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
require_cmd sha256sum
require_cmd tar
require_cmd chmod
require_cmd mkdir

# --- Modo --check (reporta transición sin instalar) -------------------
CHECK_MODE=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --check) CHECK_MODE=1; shift ;;
        *) shift ;;
    esac
done

# --- Selección de arquitectura -------------------------------------------
# Mapea uname -m al sufijo de arquitectura de los assets del release
# (build-linux-x64 → *-x86_64-linux.tar.gz, build-linux-arm64 → *-arm64-linux.tar.gz).
machine="$(uname -m)"
case "$machine" in
    x86_64|amd64)
        ASSET_ARCH="x86_64"
        ;;
    aarch64|arm64)
        ASSET_ARCH="arm64"
        ;;
    *)
        fail "arquitectura no soportada: $machine (ai-voice-interconnector publica x86_64 y arm64 para Linux)"
        ;;
esac
log "Arquitectura detectada: $machine -> $ASSET_ARCH"


# --- Resolver el release y elegir los assets ------------------------------
log "Resolviendo el último release de $REPO..."
release_json="$(curl -fsSL "$API_URL")" || fail "no se pudo consultar $API_URL"

# Extrae las URLs de descarga de los assets sin depender de jq (no siempre
# está instalado): parseo de línea con grep/sed sobre el JSON de la API.
archive_url="$(printf '%s' "$release_json" \
    | grep -o "\"browser_download_url\": *\"[^\"]*-${ASSET_ARCH}-linux\.tar\.gz\"" \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"
sums_url="$(printf '%s' "$release_json" \
    | grep -o '"browser_download_url": *"[^"]*SHA256SUMS\.txt"' \
    | sed -E 's/.*"(https:[^"]+)"/\1/' \
    | head -n1)"

[ -n "$archive_url" ] || fail "no se encontró un tar.gz de $ASSET_ARCH para Linux en el último release"
[ -n "$sums_url" ] || fail "no se encontró SHA256SUMS.txt en el último release"

archive_name="$(basename "$archive_url")"
log "Asset seleccionado: $archive_name"

# Extrae tag_name del JSON de la API (para modo --check), sin prefijo v.
latest_tag="$(printf '%s' "$release_json" | grep -o '"tag_name": *"[^"]*"' | sed -E 's/.*"([^"]+)"/\1/' | head -n1)"
latest_tag="$(printf '%s' "$latest_tag" | sed -E 's/^v//')"

# --- Modo --check: reporta transición sin instalar -------------------
if [ "$CHECK_MODE" = "1" ]; then
    if command -v ai-voice-interconnector >/dev/null 2>&1; then
        current="$(ai-voice-interconnector --version | awk '{print $2}')"
        if [ "$current" = "$latest_tag" ]; then
            log "Ya estás en la versión $latest_tag"
        else
            log "$current → $latest_tag"
        fi
    else
        log "no instalado → $latest_tag"
    fi
    exit 0
fi

# --- glibc: guard de versión mínima ----------------------------------------
# El binario se compila sobre glibc 2.35 (runner base Ubuntu 22.04); crt-static
# no enlaza glibc estáticamente en el target gnu, así que en distros más antiguas
# no arranca. Detectarlo aquí evita instalar un binario que fallaría en el primer
# uso: se aborta encaminando a la compilación desde fuente. Si la versión no puede
# parsearse se continúa: es preferible no bloquear a ciegas sobre un parseo fallido.
# Piso declarado UNA SOLA VEZ en scripts/build_utils.py (GLIBC_FLOOR = (2, 35)).
# Mantener ambas variables sincronizadas con esa constante; el test
# tests/test_pin_consistency.py.TestGlibcFloorConsistency vigila la coincidencia.
GLIBC_FLOOR_MAJOR=2
GLIBC_FLOOR_MINOR=35
if command -v ldd >/dev/null 2>&1; then
    glibc_version="$(ldd --version 2>/dev/null | head -n1 | grep -o '[0-9]\+\.[0-9]\+$' || true)"
    if [ -n "$glibc_version" ]; then
        glibc_major="$(printf '%s' "$glibc_version" | cut -d. -f1)"
        glibc_minor="$(printf '%s' "$glibc_version" | cut -d. -f2)"
        if [ "$glibc_major" -lt "$GLIBC_FLOOR_MAJOR" ] || { [ "$glibc_major" -eq "$GLIBC_FLOOR_MAJOR" ] && [ "$glibc_minor" -lt "$GLIBC_FLOOR_MINOR" ]; }; then
            log "glibc $glibc_version detectada: el binario requiere glibc >= ${GLIBC_FLOOR_MAJOR}.${GLIBC_FLOOR_MINOR} y no funcionaría en este sistema."
            log "Alternativa: compila desde la fuente (docs/BUILD.md)."
            fail "glibc insuficiente ($glibc_version < ${GLIBC_FLOOR_MAJOR}.${GLIBC_FLOOR_MINOR})"
        fi
    fi
fi

# --- Descarga y verificación de checksum ----------------------------------
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

log "Descargando $archive_name..."
curl -fsSL -o "$work_dir/$archive_name" "$archive_url" || fail "descarga del archivo fallida"

log "Descargando SHA256SUMS.txt..."
curl -fsSL -o "$work_dir/SHA256SUMS.txt" "$sums_url" || fail "descarga de SHA256SUMS.txt fallida"

log "Verificando checksum..."
( cd "$work_dir" && grep "$archive_name\$" SHA256SUMS.txt | sha256sum -c - ) \
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

# --- Integración de PATH per-user -----------------------------------------
# El `setup` del binario Rust ya no integra el PATH (solo provisiona modelos):
# el script crea el symlink en ~/.local/bin él mismo (espejo de install-macos.sh).
link_dir="${HOME}/.local/bin"
link="$link_dir/ai-voice-interconnector"
mkdir -p "$link_dir"
ln -sf "$target" "$link"
log "Symlink creado: $link -> $target"

# ~/.local/bin no siempre está en el PATH: avisa sin mutar los dotfiles del usuario.
case ":${PATH:-}:" in
    *":$link_dir:"*)
        ;;
    *)
        log ""
        log "AVISO: $link_dir no está en tu PATH."
        log "Añade esta línea a tu shell profile (~/.bashrc o ~/.profile) y reinicia la terminal:"
        log '    export PATH="$HOME/.local/bin:$PATH"'
        ;;
esac

# --- Provisión del modelo -------------------------------------------------
log ""
log "Ejecutando 'ai-voice-interconnector setup' (ofrece descargar el modelo de voz)..."
"$target" setup

log "Instalación completa."
