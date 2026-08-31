# Smoke-test de install-linux.sh (bats-core): mockea curl/uname/sha256sum/ldd
# por PATH, sin red ni GitHub real. Cubre selección de arquitectura, elección
# del asset correcto, aborto ante checksum corrupto y guard de glibc mínima
# (docs/SELF-HOSTED-INSTALL.md).
#
# Ejecutar: bats tests/installer/install-linux.bats

setup() {
    REPO_ROOT="$(cd "$(dirname "$BATS_TEST_FILENAME")/../.." && pwd)"
    INSTALL_SH="$REPO_ROOT/install-linux.sh"

    WORK="$(mktemp -d)"
    export HOME="$WORK/home"
    mkdir -p "$HOME"

    MOCK_BIN="$WORK/bin"
    mkdir -p "$MOCK_BIN"
    export PATH="$MOCK_BIN:$PATH"

    # Archivo tar.gz falso con layout plano: un binario `ai-voice-interconnector`
    # que es un script shell válido (para que "$target" setup, última línea de
    # install-linux.sh, se ejecute sin error de formato ejecutable) más un
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
    FAKE_SHA256="$(sha256sum "$ASSET_TARBALL" | cut -d' ' -f1)"

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

# Instala un mock de `uname` que responde $1 a `uname -m`.
mock_uname() {
    local machine="$1"
    cat > "$MOCK_BIN/uname" <<EOF
#!/bin/sh
if [ "\$1" = "-m" ]; then echo "$machine"; fi
EOF
    chmod +x "$MOCK_BIN/uname"
}

# Instala un mock de `curl` que sirve un release con un único asset .tar.gz
# por arquitectura ($1 = "x86_64" o "arm64") + SHA256SUMS.txt calculado
# sobre el tar.gz falso. $2 opcional: si es "corrupt", el checksum publicado
# no coincide con el contenido real (para el caso de aborto).
mock_curl() {
    local arch="$1"
    local mode="${2:-ok}"
    local asset_name="ai-voice-interconnector-1.0.0-${arch}-linux.tar.gz"
    local published_sha="$FAKE_SHA256"
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

@test "selecciona el asset x86_64 cuando uname -m devuelve x86_64" {
    mock_uname x86_64
    mock_curl x86_64

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-1.0.0-x86_64-linux.tar.gz"* ]]
    # El binario se extrae con nombre limpio (sin sufijo de arquitectura).
    [ -x "$HOME/.local/opt/ai-voice-interconnector/ai-voice-interconnector" ]
    # Y la entrada de PATH en ~/.local/bin apunta a él (symlink en Linux; en
    # hosts sin symlinks, ln cae a copia, así que se verifica existencia).
    [ -e "$HOME/.local/bin/ai-voice-interconnector" ]
}

@test "selecciona el asset arm64 cuando uname -m devuelve aarch64" {
    mock_uname aarch64
    mock_curl arm64

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-1.0.0-arm64-linux.tar.gz"* ]]
    [ -x "$HOME/.local/opt/ai-voice-interconnector/ai-voice-interconnector" ]
    [ -e "$HOME/.local/bin/ai-voice-interconnector" ]
}

# Instala un mock de `ldd` cuyo `--version` reporta la glibc $1.
mock_ldd() {
    local version="$1"
    cat > "$MOCK_BIN/ldd" <<EOF
#!/bin/sh
if [ "\$1" = "--version" ]; then echo "ldd (GNU libc) $version"; fi
EOF
    chmod +x "$MOCK_BIN/ldd"
}

@test "glibc < 2.35 aborta encaminando a la compilación desde fuente" {
    mock_uname x86_64
    mock_curl x86_64
    mock_ldd 2.31

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"glibc"* ]]
    [[ "$output" == *"BUILD.md"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}

@test "glibc >= 2.35 no bloquea la instalación" {
    mock_uname x86_64
    mock_curl x86_64
    mock_ldd 2.35

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
}

@test "arquitectura no soportada aborta con error" {
    mock_uname riscv64
    mock_curl x86_64

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"arquitectura no soportada"* ]]
}

@test "aborta y no instala nada si el checksum no coincide" {
    mock_uname x86_64
    mock_curl x86_64 corrupt

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}

@test "al actualizar limpia el directorio anterior y deja solo el nuevo contenido" {
    mock_uname x86_64
    mock_curl x86_64

    # Pre-siembra un archivo huérfano de una versión anterior en el directorio
    # de instalación, como si viniera de una instalación previa.
    install_dir="$HOME/.local/opt/ai-voice-interconnector"
    mkdir -p "$install_dir"
    orphan="$install_dir/ai-voice-interconnector-0.9.0-x86_64.AppImage"
    printf 'viejo' > "$orphan"

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El archivo huérfano fue eliminado (rm -rf del directorio antes de extraer).
    [ ! -e "$orphan" ]
    # El binario nuevo existe con nombre limpio y es ejecutable.
    [ -x "$install_dir/ai-voice-interconnector" ]
    # No quedan AppImages residuales.
    count="$(ls "$install_dir"/*.AppImage 2>/dev/null | wc -l)"
    [ "$count" -eq 0 ]
}

@test "--check reporta transición cuando hay versión nueva" {
    mock_uname x86_64
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
{"assets":[{"browser_download_url":"https://example.invalid/ai-voice-interconnector-1.0.0-x86_64-linux.tar.gz"},{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}],"tag_name":"v2.0.0"}
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
    mock_uname x86_64
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
{"assets":[{"browser_download_url":"https://example.invalid/ai-voice-interconnector-1.0.0-x86_64-linux.tar.gz"},{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}],"tag_name":"v1.0.0"}
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
