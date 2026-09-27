# Ayudas del arnés local para install.bats (solo pruebas, POSIX sh + bats).
#
# El servidor falso (support/serve.py) sirve por HTTPS con certificado propio
# los assets por target y un SHA256SUMS.txt coherente o corrupto, porque el
# bootstrap fija `curl --proto '=https'` y el http plano no llegaría al
# servidor. `CURL_CA_BUNDLE` hace que el curl real confíe en la CA de prueba
# sin relajar el TLS del bootstrap. La versión de prueba es 9.9.9 en todos
# los casos: lo inexistente (8.8.8) falla con error de red, lo que demuestra
# qué resolución se eligió sin inspeccionar el código.
#
# El falso `ai-voice-interconnector` empaquetado en cada asset registra
# `self install ...` en AVI_FAKE_LOG, exige terminal sin --yes (emula la
# confirmación) y con AVI_FAKE_MODE=noexec no arranca (binario incompatible).

# Versión de todos los assets del arnés.
HARNESS_VERSION="9.9.9"

# Escribe el falso binario (doble de `self install`) en $1.
harness_write_fake_binary() {
    dest="$1"
    cat > "$dest" <<'FAKE'
#!/bin/sh
# Doble del binario instalado (solo pruebas): --version arranca; `self
# install` registra sus argumentos y, sin --yes, exige terminal como haría la
# confirmación real; con AVI_FAKE_MODE=noexec nada arranca (incompatible).
if [ "${AVI_FAKE_MODE:-ok}" = "noexec" ]; then exit 3; fi
if [ "${1:-}" = "--version" ]; then
    printf 'ai-voice-interconnector %s\n' "${AVI_FAKE_VERSION:-9.9.9}"
    exit 0
fi
printf '%s\n' "$*" >> "${AVI_FAKE_LOG:-/dev/null}"
tiene_yes=0
for a in "$@"; do if [ "$a" = "--yes" ]; then tiene_yes=1; fi; done
if [ "$tiene_yes" = "1" ]; then exit 0; fi
if [ -t 0 ]; then exit 0; fi
echo "se necesita confirmación interactiva o --yes" >&2
exit 42
FAKE
    chmod +x "$dest"
}

# Construye en $1/v$HARNESS_VERSION los tres tar.gz por target (x86_64-linux,
# arm64-linux, arm64-macos), cada uno con el falso binario.
harness_make_assets() {
    root="$1"
    staged="$(mktemp -d)"
    harness_write_fake_binary "$staged/ai-voice-interconnector"
    mkdir -p "$root/v$HARNESS_VERSION"
    ( cd "$staged" && tar -czf "$root/v$HARNESS_VERSION/ai-voice-interconnector-$HARNESS_VERSION-x86_64-linux.tar.gz" ai-voice-interconnector )
    ( cd "$staged" && tar -czf "$root/v$HARNESS_VERSION/ai-voice-interconnector-$HARNESS_VERSION-arm64-linux.tar.gz" ai-voice-interconnector )
    ( cd "$staged" && tar -czf "$root/v$HARNESS_VERSION/ai-voice-interconnector-$HARNESS_VERSION-arm64-macos.tar.gz" ai-voice-interconnector )
    rm -rf "$staged"
}

# Suma disponible (sha256sum o shasum, como el bootstrap).
harness_sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d' ' -f1
    else
        shasum -a 256 "$1" | cut -d' ' -f1
    fi
}

# Escribe SHA256SUMS.txt en $1/v$HARNESS_VERSION. $2: ok (coherente),
# corrupt (hash falso para el asset de $3) o missing (sin línea para $3).
harness_write_sums() {
    root="$1"; mode="${2:-ok}"; target="${3:-}"
    dir="$root/v$HARNESS_VERSION"
    : > "$dir/SHA256SUMS.txt"
    for asset in "$dir"/ai-voice-interconnector-*.tar.gz; do
        name="$(basename "$asset")"
        sum="$(harness_sha256_file "$asset")"
        if [ "$mode" = "corrupt" ] && [ "$name" = "$target" ]; then
            sum="00000000000000000000000000000000000000000000000000000000000000ff"
        fi
        if [ "$mode" = "missing" ] && [ "$name" = "$target" ]; then
            continue
        fi
        printf '%s  %s\n' "$sum" "$name" >> "$dir/SHA256SUMS.txt"
    done
}

# Arranca el servidor HTTPS sobre $1 con el certificado de $2; exporta
# AVI_DOWNLOAD_BASE_URL y CURL_CA_BUNDLE. Guarda el PID en SERVER_PID.
harness_start_server() {
    root="$1"; certdir="$2"
    command -v curl >/dev/null 2>&1 || { echo "el arnés necesita curl" >&2; return 1; }
    SERVER_PORT_FILE="$WORK/port"
    SERVER_LOG="$WORK/requests.log"
    : > "$SERVER_LOG"
    python3 "$SUPPORT/serve.py" --dir "$root" --port-file "$SERVER_PORT_FILE" \
        --request-log "$SERVER_LOG" --cert "$certdir/server.pem" --key "$certdir/server-key.pem" \
        >"$WORK/server.out" 2>"$WORK/server.err" &
    SERVER_PID="$!"
    for _ in $(seq 1 50); do
        if [ -s "$SERVER_PORT_FILE" ]; then
            port="$(cat "$SERVER_PORT_FILE")"
            if curl -ksSf -o /dev/null "https://127.0.0.1:$port/" 2>/dev/null; then
                break
            fi
        fi
        sleep 0.2
    done
    port="$(cat "$SERVER_PORT_FILE" 2>/dev/null || true)"
    if [ -z "${port:-}" ] || ! curl -ksSf -o /dev/null "https://127.0.0.1:$port/" 2>/dev/null; then
        echo "el servidor falso no arrancó" >&2
        cat "$WORK/server.err" >&2 || true
        return 1
    fi
    export AVI_DOWNLOAD_BASE_URL="https://127.0.0.1:$port"
    export CURL_CA_BUNDLE="$certdir/ca.pem"
}

# Detiene el servidor arrancado con harness_start_server.
harness_stop_server() {
    if [ -n "${SERVER_PID:-}" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
}

# Reubica las cuatro raíces (§7) y el registro del doble a temporales de $1.
harness_relocate() {
    base="$1"
    export AVI_INSTALL_DIR="$base/install"
    export AVI_BIN_DIR="$base/bin"
    export AVI_DATA_DIR="$base/data"
    export AVI_CACHE_DIR="$base/cache"
    export AVI_FAKE_LOG="$base/fake.log"
    mkdir -p "$AVI_INSTALL_DIR" "$AVI_BIN_DIR" "$AVI_DATA_DIR" "$AVI_CACHE_DIR"
    : > "$AVI_FAKE_LOG"
}

# Sombra `uname`: $1 = máquina (`uname -m`), $2 = sistema (`uname -s`).
mock_uname() {
    machine="$1"; os="${2:-Linux}"
    cat > "$MOCK_BIN/uname" <<EOF
#!/bin/sh
case "\$1" in
    -m) echo "$machine" ;;
    -s) echo "$os" ;;
    *) command -p uname "\$@" ;;
esac
EOF
    chmod +x "$MOCK_BIN/uname"
}

# Sombra `sysctl` para que `sysctl -n hw.optional.arm64` responda $1.
mock_sysctl() {
    value="$1"
    cat > "$MOCK_BIN/sysctl" <<EOF
#!/bin/sh
echo "$value"
EOF
    chmod +x "$MOCK_BIN/sysctl"
}
