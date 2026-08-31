#!/bin/sh
# Wrapper de auto-actualización para ai-voice-interconnector (Linux/macOS).
#
# Uso:
#   sh upgrade-ai-voice-interconnector.sh          # actualiza (re-ejecuta el one-liner)
#   sh upgrade-ai-voice-interconnector.sh --check  # solo reporta la transición
#
# Detecta el SO (uname -s), obtiene la versión instalada y la última,
# reporta la transición (de → a), y re-ejecuta el one-liner del SO
# correspondiente salvo que se pase --check.

set -eu
script_dir="$(cd "$(dirname "$0")" && pwd)"

REPO="CristianRojas-SoftwareEngineer/AI-Voice-InterConnector"

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "falta el comando requerido: $1"
}

fail() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 1
}

require_cmd curl
require_cmd uname

OS="$(uname -s)"
case "$OS" in
    Linux)     ONE_LINER="install-linux.sh" ;;
    Darwin)    ONE_LINER="install-macos.sh" ;;
    *)         fail "SO no soportado: $OS" ;;
esac

# --- Modo --check (reporta transición sin instalar) -------------------
CHECK_MODE=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --check) CHECK_MODE=1; shift ;;
        *) shift ;;
    esac
done

# Obtener versión instalada (vacío si no está instalado).
installed="$(command -v ai-voice-interconnector >/dev/null 2>&1 && ai-voice-interconnector --version | awk '{print $2}' || true)"

# Obtener tag_name del release más reciente.
release_json="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest")" || fail "no se pudo consultar la API de GitHub"
latest_tag="$(printf '%s' "$release_json" | grep -o '"tag_name": *"[^"]*"' | sed -E 's/.*"([^"]+)"/\1/' | head -n1)"
latest_tag="$(printf '%s' "$latest_tag" | sed -E 's/^v//')"

if [ -n "$installed" ]; then
    if [ "$installed" = "$latest_tag" ]; then
        printf 'Ya estás en la versión %s\n' "$latest_tag"
    else
        printf '%s → %s\n' "$installed" "$latest_tag"
    fi
else
    printf 'no instalado → %s\n' "$latest_tag"
fi

if [ "$CHECK_MODE" = "1" ]; then
    exit 0
fi

# Re-ejecutar el one-liner del SO correspondiente (busca en PATH,
# o en el directorio del wrapper como respaldo).
ONE_LINER_PATH="$(command -v "$ONE_LINER" || true)"
sh "${ONE_LINER_PATH:-$script_dir/$ONE_LINER}"
