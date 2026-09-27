#!/bin/sh
# Bootstrap de primera instalación de ai-voice-interconnector (Linux y macOS).
#
# Uso con la última versión:
#   curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
# Uso con opciones (como archivo o tras `sh -s --`):
#   curl -fsSL <url> | sh -s -- --version 0.24.0 --no-setup --yes
#
# Flujo (§9.2): detecta el target, resuelve la versión, crea un staging hermano
# del programa, descarga por HTTPS, verifica el hash con comparación exacta,
# comprueba que el binario arranca y delega en `self install`. Sin `--check`
# (decisión d): con el binario instalado se usa `self update --check`. El PATH,
# los modelos y las rutas de estado los decide `self install`, no este script.

set -eu
umask 077

repo="CristianRojas-SoftwareEngineer/AI-Voice-InterConnector"
app="ai-voice-interconnector"

# Versión estampada al publicar el release (la sustituye c3-05, una sola vez).
stamped_version="__AVI_STAMPED_VERSION__"

log() { printf '%s\n' "$*" >&2; }
fail() { log "ERROR: $1"; exit "${2:-1}"; }
have() { command -v "$1" >/dev/null 2>&1; }

print_help() {
    cat >&2 <<'EOF'
Uso: install.sh [--version X.Y.Z] [--no-setup] [--no-modify-path] [--yes]
Variables equivalentes: AVI_VERSION, AVI_NO_SETUP=1, AVI_NO_MODIFY_PATH=1,
AVI_YES=1, AVI_DOWNLOAD_BASE_URL. Sin --check: usa `self update --check`.
EOF
}

# Entradas (§9.2): la opción manda; si falta, vale la variable de entorno.
opt_version="${AVI_VERSION:-}"
opt_no_setup=0; opt_no_modify_path=0; opt_yes=0
case "${AVI_NO_SETUP:-}" in 1|true|yes|True|Yes|TRUE|YES) opt_no_setup=1 ;; esac
case "${AVI_NO_MODIFY_PATH:-}" in 1|true|yes|True|Yes|TRUE|YES) opt_no_modify_path=1 ;; esac
case "${AVI_YES:-}" in 1|true|yes|True|Yes|TRUE|YES) opt_yes=1 ;; esac
while [ "$#" -gt 0 ]; do
    case "$1" in
        --version) [ "$#" -ge 2 ] || fail "[usage_error] --version necesita un valor X.Y.Z." 2; opt_version="$2"; shift 2 ;;
        --version=*) opt_version="${1#--version=}"; shift ;;
        --no-setup) opt_no_setup=1; shift ;;
        --no-modify-path) opt_no_modify_path=1; shift ;;
        --yes) opt_yes=1; shift ;;
        -h|--help) print_help; exit 0 ;;
        --) shift; break ;;
        -*) fail "[usage_error] opción desconocida: $1." 2 ;;
        *) fail "[usage_error] argumento inesperado: $1." 2 ;;
    esac
done

# Privilegios (§9.1): instalación per-user; con sudo se aborta, root puro vale.
if [ "$(id -u 2>/dev/null || printf '1000')" = "0" ] && [ -n "${SUDO_USER:-}" ]; then
    fail "no ejecutes este instalador con sudo: la instalación es per-user y acabaría en el perfil de root."
fi

# Detección del target (§3); en macOS manda sysctl aunque haya Rosetta.
os="$(uname -s)"
machine="$(uname -m)"
case "$os" in
    Linux)
        case "$machine" in
            x86_64|amd64) target="x86_64-unknown-linux-gnu"; asset_arch="x86_64"; asset_os="linux" ;;
            aarch64|arm64) target="aarch64-unknown-linux-gnu"; asset_arch="arm64"; asset_os="linux" ;;
            *) fail "[unsupported_platform] arquitectura no soportada: Linux/$machine (solo x86_64 y arm64). Alternativa: compila desde la fuente (docs/BUILD.md)." ;;
        esac
        ;;
    Darwin)
        if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || printf '0')" = "1" ]; then
            target="aarch64-apple-darwin"; asset_arch="arm64"; asset_os="macos"
        else
            fail "[unsupported_platform] Mac Intel no soportado (solo Apple Silicon). Alternativa: compila desde la fuente (docs/BUILD.md)."
        fi
        ;;
    *) fail "[unsupported_platform] sistema no soportado: $os (solo Linux y macOS; en Windows usa install.ps1)." ;;
esac

# Requisitos (§9.2): POSIX sh más descarga, hash, extracción y temporales.
for cmd in mktemp tar tr dirname; do have "$cmd" || fail "falta el comando requerido: $cmd."; done
have curl || have wget || fail "se necesita curl o wget para descargar."
have sha256sum || have shasum || fail "se necesita sha256sum o shasum para verificar."

fetch() { # $1=url, $2=destino
    if have curl; then curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"; else wget -q -O "$2" "$1"; fi
}

# Última estable sin API REST: se sigue la redirección de releases/latest.
resolve_latest_redirect() {
    effective="$(curl -fsSL -o /dev/null -w '%{url_effective}' --proto '=https' --tlsv1.2 "https://github.com/$repo/releases/latest" 2>/dev/null || true)"
    tag="${effective##*/tag/}"
    case "$tag" in "$effective"|"") return 1 ;; esac
    printf '%s' "${tag#v}"
}

# Respaldo con la API sin parsear JSON: solo se extrae tag_name con
# expansiones de la shell (sin grep, sed, awk ni expresiones regulares).
resolve_latest_api() {
    if have curl; then body="$(curl -fsSL --proto '=https' --tlsv1.2 "https://api.github.com/repos/$repo/releases/latest" 2>/dev/null || true)";
    else body="$(wget -q -O - "https://api.github.com/repos/$repo/releases/latest" 2>/dev/null || true)"; fi
    rest="${body#*tag_name}"
    [ "$rest" != "$body" ] || return 1
    rest="${rest#*:}"
    while [ -n "$rest" ]; do case "$rest" in [v0-9]*) break ;; *) rest="${rest#?}" ;; esac; done
    [ -n "$rest" ] || return 1
    tag=""
    while [ -n "$rest" ]; do
        ch="${rest%"${rest#?}"}"
        case "$ch" in [0-9A-Za-z.-]) tag="$tag$ch"; rest="${rest#?}" ;; *) break ;; esac
    done
    [ -n "$tag" ] || return 1
    printf '%s' "${tag#v}"
}

# Resolución de versión (§9.2 paso 3): opción o variable, estampada, latest.
version="${opt_version#v}"
case "$version" in "") case "$stamped_version" in ""|__AVI_*) version="" ;; *) version="$stamped_version" ;; esac ;; esac
case "$version" in
    "") version="$(resolve_latest_redirect || resolve_latest_api || true)"
        [ -n "$version" ] || fail "[network_error] no se pudo resolver la última versión (sin red o sin GitHub). Fija una con --version X.Y.Z o AVI_VERSION." ;;
esac
case "$version" in ""|*[!0-9.]*|*..*|.*|*.) fail "[usage_error] versión inválida: '$version' (se espera X.Y.Z)." 2 ;; esac

# Staging hermano del programa (mismo volumen), solo para el usuario.
program_dir="${AVI_INSTALL_DIR:-${HOME:-}/.local/opt/$app}"
[ -n "$program_dir" ] && [ "$program_dir" != "/.local/opt/$app" ] || fail "no se puede situar el staging sin HOME ni AVI_INSTALL_DIR."
program_parent="$(dirname "$program_dir")"
mkdir -p "$program_parent" || fail "no se pudo crear $program_parent."
staging="$(mktemp -d "$program_parent/.ai-voice-interconnector-staging-XXXXXX")" || fail "no se pudo crear el staging."
chmod 700 "$staging"
trap 'rm -rf "$staging"' EXIT INT TERM

base="${AVI_DOWNLOAD_BASE_URL:-https://github.com/$repo/releases/download}"
archive="$app-$version-$asset_arch-$asset_os.tar.gz"
log "Instalando $app $version ($target)..."
fetch "$base/v$version/$archive" "$staging/$archive" || fail "[network_error] descarga fallida: $archive."
fetch "$base/v$version/SHA256SUMS.txt" "$staging/SHA256SUMS.txt" || fail "[network_error] descarga fallida: SHA256SUMS.txt."

# Verificación exacta: el nombre debe coincidir cadena a cadena, sin regex.
expected=""
while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in ""|\#*) continue ;; esac
    # shellcheck disable=SC2086
    set -- $line
    entry="${2:-}"; entry="${entry#"*"}"
    if [ "$entry" = "$archive" ]; then expected="${1:-}"; break; fi
done < "$staging/SHA256SUMS.txt"
[ -n "$expected" ] || fail "[checksum_mismatch] SHA256SUMS.txt no contiene ninguna línea para $archive; instalación abortada."
if have sha256sum; then actual="$(sha256sum "$staging/$archive" || true)"; else actual="$(shasum -a 256 "$staging/$archive" || true)"; fi
actual="${actual%% *}"
expected="$(printf '%s' "$expected" | tr 'A-Z' 'a-z')"
actual="$(printf '%s' "$actual" | tr 'A-Z' 'a-z')"
[ "$actual" = "$expected" ] || fail "[checksum_mismatch] el checksum de $archive no coincide con SHA256SUMS.txt; instalación abortada."
log "Checksum verificado: $archive"

tar -xzf "$staging/$archive" -C "$staging" || fail "no se pudo extraer $archive."
bin="$staging/$app"
[ -x "$bin" ] || fail "[bundle_invalid] el archivo no contiene el binario esperado: $app."
# Compatibilidad comprobada, no inferida (decisión b): si no arranca se
# diagnostica sin parsear la glibc y lo instalado queda intacto.
if ! "$bin" --version >/dev/null 2>&1; then
    log "ERROR [binary_incompatible]: el binario descargado ($target) no arranca en este sistema."
    log "Causa probable en Linux: glibc insuficiente (se requiere glibc >= 2.35), musl (Alpine) o userland de 32 bits."
    log "Alternativa: compila desde la fuente siguiendo docs/BUILD.md. La instalación existente no se ha modificado."
    exit 1
fi

# Delegación en el binario nuevo (§9.2 paso 9); con tubería, stdin va a /dev/tty
# cuando se puede abrir (existe el nodo pero sin terminal rectora no se abre).
set -- self install
[ "$opt_no_setup" = "1" ] && set -- "$@" --no-setup
[ "$opt_no_modify_path" = "1" ] && set -- "$@" --no-modify-path
[ "$opt_yes" = "1" ] && set -- "$@" --yes
code=0
if [ ! -t 0 ] && ( : < /dev/tty ) 2>/dev/null; then
    if "$bin" "$@" < /dev/tty; then code=0; else code=$?; fi
else
    if "$bin" "$@"; then code=0; else code=$?; fi
fi
rm -rf "$staging"
trap - EXIT INT TERM
exit "$code"
