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

    # Contenido fijo del "AppImage" falso: un script shell válido, para que
    # "$final_path" setup (última línea de install-linux.sh) se ejecute sin error
    # de formato ejecutable, en vez de un ELF real.
    FAKE_APPIMAGE_CONTENT='#!/bin/sh
echo "fake ai-voice-interconnector $*"
'
    FAKE_SHA256="$(printf '%s' "$FAKE_APPIMAGE_CONTENT" | sha256sum | cut -d' ' -f1)"
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

# Instala un mock de `curl` que sirve un release con un único asset .AppImage
# por arquitectura ($1 = "x86_64" o "arm64") + SHA256SUMS.txt calculado
# sobre FAKE_APPIMAGE_CONTENT. $2 opcional: si es "corrupt", el checksum
# publicado no coincide con el contenido real (para el caso de aborto).
mock_curl() {
    local arch="$1"
    local mode="${2:-ok}"
    local asset_name="ai-voice-interconnector-1.0.0-${arch}.AppImage"
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
]}
JSON
        ;;
    *${asset_name})
        printf '%s' '$FAKE_APPIMAGE_CONTENT' > "\$out"
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
    [[ "$output" == *"ai-voice-interconnector-1.0.0-x86_64.AppImage"* ]]
    [ -f "$HOME/.local/opt/ai-voice-interconnector/ai-voice-interconnector-1.0.0-x86_64.AppImage" ]
}

@test "selecciona el asset arm64 cuando uname -m devuelve aarch64" {
    mock_uname aarch64
    mock_curl arm64

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-1.0.0-arm64.AppImage"* ]]
    [ -f "$HOME/.local/opt/ai-voice-interconnector/ai-voice-interconnector-1.0.0-arm64.AppImage" ]
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

@test "glibc < 2.35 aborta encaminando a PyPI/fuente" {
    mock_uname x86_64
    mock_curl x86_64
    mock_ldd 2.31

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"glibc"* ]]
    [[ "$output" == *"PyPI"* ]]
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

@test "al actualizar elimina el AppImage anterior y deja solo el nuevo" {
    mock_uname x86_64
    mock_curl x86_64

    # Pre-siembra un AppImage de una versión anterior en el directorio de
    # instalación, como si viniera de una instalación previa.
    install_dir="$HOME/.local/opt/ai-voice-interconnector"
    mkdir -p "$install_dir"
    old_appimage="$install_dir/ai-voice-interconnector-0.9.0-x86_64.AppImage"
    printf 'viejo' > "$old_appimage"
    chmod +x "$old_appimage"

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El viejo fue eliminado y el nuevo existe y es ejecutable.
    [ ! -e "$old_appimage" ]
    new_appimage="$install_dir/ai-voice-interconnector-1.0.0-x86_64.AppImage"
    [ -f "$new_appimage" ]
    [ -x "$new_appimage" ]
    # Solo queda un AppImage en el directorio.
    count="$(ls "$install_dir"/ai-voice-interconnector-*.AppImage | wc -l)"
    [ "$count" -eq 1 ]
}
