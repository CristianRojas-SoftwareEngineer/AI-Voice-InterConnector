# Smoke-test de install-macos.sh (bats-core): mockea curl/uname/xattr por PATH,
# sin red ni tar.gz real de GitHub (shasum y tar son reales). Cubre el guard de
# arquitectura, la selección del asset, el aborto ante checksum corrupto, la
# instalación feliz y el reemplazo de una instalación anterior
# (docs/SELF-HOSTED-INSTALL.md).
#
# El job CI `test-installer-macos` lo ejecuta en el executor macOS real; los
# mocks permiten correrlo en cualquier host con shasum y tar.
#
# Ejecutar: bats tests/installer/install-macos.bats

setup() {
    REPO_ROOT="$(cd "$(dirname "$BATS_TEST_FILENAME")/../.." && pwd)"
    INSTALL_SH="$REPO_ROOT/install-macos.sh"

    WORK="$(mktemp -d)"
    export HOME="$WORK/home"
    mkdir -p "$HOME"

    MOCK_BIN="$WORK/bin"
    mkdir -p "$MOCK_BIN"
    export PATH="$MOCK_BIN:$PATH"

    # Archivo tar.gz falso con layout plano: un binario `ai-voice-interconnector`
    # que es un script shell válido (para que "$target" setup, última línea de
    # install-macos.sh, se ejecute sin error de formato ejecutable) más un
    # documento de licencia, imitando el asset real. Se construye una sola vez
    # para que su SHA-256 sea determinista.
    stage="$WORK/stage"
    mkdir -p "$stage"
    cat > "$stage/ai-voice-interconnector" <<'BIN'
#!/bin/sh
echo "fake ai-voice-interconnector $*"
BIN
    chmod +x "$stage/ai-voice-interconnector"
    printf 'GPLv3\n' > "$stage/LICENSE"
    ASSET_TARBALL="$WORK/asset.tar.gz"
    tar -czf "$ASSET_TARBALL" -C "$stage" .
    export ASSET_TARBALL
    FAKE_SHA="$(shasum -a 256 "$ASSET_TARBALL" | cut -d' ' -f1)"

    # Mock de `ai-voice-interconnector` para modo --check: reporta
    # "ai-voice-interconnector 1.0.0" a --version.
    cat > "$MOCK_BIN/ai-voice-interconnector" <<'MOCK'
#!/bin/sh
echo "ai-voice-interconnector 1.0.0"
MOCK
    chmod +x "$MOCK_BIN/ai-voice-interconnector"
}

teardown() {
    rm -rf "$WORK"
}

# Mock de `uname`: responde $1 a `uname -m`.
mock_uname() {
    local machine="$1"
    cat > "$MOCK_BIN/uname" <<EOF
#!/bin/sh
if [ "\$1" = "-m" ]; then echo "$machine"; fi
EOF
    chmod +x "$MOCK_BIN/uname"
}

# Mock de `curl`: sirve un release con un único tar.gz de arm64 + SHA256SUMS.txt.
# $1 opcional: si es "corrupt", el checksum publicado no coincide con el tar.gz.
mock_curl() {
    local mode="${1:-ok}"
    local asset_name="ai-voice-interconnector-1.0.0-arm64-macos.tar.gz"
    local published_sha="$FAKE_SHA"
    if [ "$mode" = "corrupt" ]; then
        published_sha="0000000000000000000000000000000000000000000000000000000000ff"
    fi

    cat > "$MOCK_BIN/curl" <<EOF
#!/bin/sh
out=""
url=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        -o) out="\$2"; shift 2 ;;
        -fsSL) shift ;;
        http*) url="\$1"; shift ;;
        *) shift ;;
    esac
done
case "\$url" in
    *api.github.com*)
        cat <<JSON
{"assets":[
{"browser_download_url":"https://example.invalid/${asset_name}"},
{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}
],
"tag_name":"v1.0.0"}
JSON
        ;;
    *${asset_name})
        cp "$ASSET_TARBALL" "\$out"
        ;;
    *SHA256SUMS.txt)
        printf '%s  %s\n' "$published_sha" "$asset_name" > "\$out"
        ;;
esac
EOF
    chmod +x "$MOCK_BIN/curl"
}

# Mock de `xattr`: no-op (limpieza de cuarentena).
mock_xattr() {
    cat > "$MOCK_BIN/xattr" <<'EOF'
#!/bin/sh
exit 0
EOF
    chmod +x "$MOCK_BIN/xattr"
}

# Instala todos los mocks del camino feliz.
mock_all() {
    mock_uname arm64
    mock_curl "${1:-ok}"
    mock_xattr
}

@test "rechaza una arquitectura que no sea arm64" {
    mock_all
    mock_uname x86_64

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"arquitectura no soportada"* ]]
    # El rechazo debe encaminar a Mac Intel hacia la compilación desde fuente.
    [[ "$output" == *"BUILD.md"* ]]
}

@test "selecciona el asset tar.gz de arm64" {
    mock_all

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-1.0.0-arm64-macos.tar.gz"* ]]
}

@test "aborta si el checksum no coincide" {
    mock_all corrupt

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}

@test "instalación feliz: extrae el binario, crea el symlink e invoca setup" {
    mock_all

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El binario quedó extraído en el directorio de instalación per-user.
    [ -x "$HOME/.local/opt/ai-voice-interconnector/ai-voice-interconnector" ]
    # La entrada de PATH en ~/.local/bin apunta al binario extraído (symlink en
    # macOS; en hosts sin symlinks, ln cae a copia, así que se verifica existencia).
    [ -e "$HOME/.local/bin/ai-voice-interconnector" ]
    # setup fue invocado (el binario falso lo eco).
    [[ "$output" == *"fake ai-voice-interconnector setup"* ]]
}

@test "reemplaza una instalación anterior" {
    mock_all

    # Pre-siembra una instalación anterior con contenido distinguible.
    install_dir="$HOME/.local/opt/ai-voice-interconnector"
    mkdir -p "$install_dir"
    printf 'binario viejo' > "$install_dir/ai-voice-interconnector"

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El binario fue reemplazado por el nuevo (script, no "binario viejo").
    new_bin="$install_dir/ai-voice-interconnector"
    [ -x "$new_bin" ]
    ! grep -q "binario viejo" "$new_bin"
}

@test "--check reporta transición cuando hay versión nueva" {
    mock_all
    # curl responde con tag_name v2.0.0 (versión nueva respecto al mock 1.0.0).
    cat > "$MOCK_BIN/curl" <<'EOF'
#!/bin/sh
out=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        -fsSL) shift ;;
        http*) url="$1"; shift ;;
        *) shift ;;
    esac
done
case "$url" in
    *api.github.com*)
        cat <<JSON
{"assets":[{"browser_download_url":"https://example.invalid/ai-voice-interconnector-1.0.0-arm64-macos.tar.gz"},{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}],"tag_name":"v2.0.0"}
JSON
        ;;
    *) ;;
esac
EOF
    chmod +x "$MOCK_BIN/curl"

    run sh "$INSTALL_SH" --check

    [ "$status" -eq 0 ]
    [[ "$output" == *"1.0.0 → 2.0.0"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}

@test "--check reporta ya estás en la versión cuando no hay actualización" {
    mock_all
    # curl responde con tag_name v1.0.0 (misma versión del mock).
    cat > "$MOCK_BIN/curl" <<'EOF'
#!/bin/sh
out=""
url=""
while [ $# -gt 0 ]; do
    case "$1" in
        -o) out="$2"; shift 2 ;;
        -fsSL) shift ;;
        http*) url="$1"; shift ;;
        *) shift ;;
    esac
done
case "$url" in
    *api.github.com*)
        cat <<JSON
{"assets":[{"browser_download_url":"https://example.invalid/ai-voice-interconnector-1.0.0-arm64-macos.tar.gz"},{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}],"tag_name":"v1.0.0"}
JSON
        ;;
    *) ;;
esac
EOF
    chmod +x "$MOCK_BIN/curl"

    run sh "$INSTALL_SH" --check

    [ "$status" -eq 0 ]
    [[ "$output" == *"Ya estás en la versión"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}
