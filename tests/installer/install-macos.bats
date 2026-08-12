# Smoke-test de install-macos.sh (bats-core): mockea curl/uname/hdiutil/xattr/
# ditto por PATH, sin red ni .dmg real (shasum es real). Cubre el guard de
# arquitectura, la selección del asset, el aborto ante checksum corrupto, la
# instalación feliz y el reemplazo de una instalación anterior
# (docs/SELF-HOSTED-INSTALL.md).
#
# El job CI `test-installer-macos` lo ejecuta en el executor macOS real; los
# mocks permiten correrlo en cualquier host con shasum.
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

    # Contenido fijo del binario del ".app" que el mock de hdiutil materializa
    # en el mountpoint: un script shell válido, para que "$target" setup (última
    # línea de install-macos.sh) se ejecute sin error de formato ejecutable.
    FAKE_BIN_CONTENT='#!/bin/sh
echo "fake ai-voice-interconnector $*"
'
    # Contenido fijo del ".dmg" falso; su checksum se publica en SHA256SUMS.txt.
    FAKE_DMG_CONTENT='contenido binario simulado del dmg'
    FAKE_DMG_SHA="$(printf '%s' "$FAKE_DMG_CONTENT" | shasum -a 256 | cut -d' ' -f1)"
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

# Mock de `curl`: sirve un release con un único .dmg de arm64 + SHA256SUMS.txt.
# $1 opcional: si es "corrupt", el checksum publicado no coincide con el .dmg.
mock_curl() {
    local mode="${1:-ok}"
    local asset_name="ai-voice-interconnector-1.0.0-arm64.dmg"
    local published_sha="$FAKE_DMG_SHA"
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
        printf '%s' '$FAKE_DMG_CONTENT' > "\$out"
        ;;
    *SHA256SUMS.txt)
        printf '%s  %s\n' "$published_sha" "$asset_name" > "\$out"
        ;;
esac
EOF
    chmod +x "$MOCK_BIN/curl"
}

# Mock de `hdiutil`: en `attach ... -mountpoint <mp>` materializa un .app falso
# en <mp>; `detach` es no-op.
mock_hdiutil() {
    cat > "$MOCK_BIN/hdiutil" <<EOF
#!/bin/sh
action="\$1"
shift
if [ "\$action" = "attach" ]; then
    mp=""
    while [ \$# -gt 0 ]; do
        case "\$1" in
            -mountpoint) mp="\$2"; shift 2 ;;
            *) shift ;;
        esac
    done
    app="\$mp/ai-voice-interconnector-arm64.app"
    mkdir -p "\$app/Contents/MacOS"
    printf '%s' '$FAKE_BIN_CONTENT' > "\$app/Contents/MacOS/ai-voice-interconnector"
    chmod +x "\$app/Contents/MacOS/ai-voice-interconnector"
fi
exit 0
EOF
    chmod +x "$MOCK_BIN/hdiutil"
}

# Mock de `ditto`: copia recursiva src → dst (ditto src dst hace dst copia de src).
mock_ditto() {
    cat > "$MOCK_BIN/ditto" <<'EOF'
#!/bin/sh
src="$1"
dst="$2"
cp -R "$src" "$dst"
EOF
    chmod +x "$MOCK_BIN/ditto"
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
    mock_hdiutil
    mock_ditto
    mock_xattr
}

@test "rechaza una arquitectura que no sea arm64" {
    mock_all
    mock_uname x86_64

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"arquitectura no soportada"* ]]
    # El rechazo debe encaminar a Mac Intel hacia las alternativas.
    [[ "$output" == *"PyPI"* ]]
}

@test "selecciona el asset .dmg de arm64" {
    mock_all

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-1.0.0-arm64.dmg"* ]]
}

@test "aborta si el checksum no coincide" {
    mock_all corrupt

    run sh "$INSTALL_SH"

    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum"* ]]
    [ ! -d "$HOME/Applications/ai-voice-interconnector-arm64.app" ]
}

@test "instalación feliz: copia el .app, crea el symlink e invoca setup" {
    mock_all

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El .app quedó copiado a ~/Applications.
    [ -x "$HOME/Applications/ai-voice-interconnector-arm64.app/Contents/MacOS/ai-voice-interconnector" ]
    # El symlink de PATH per-user apunta al binario del .app.
    [ -L "$HOME/.local/bin/ai-voice-interconnector" ]
    # setup fue invocado (el binario falso lo eco).
    [[ "$output" == *"fake ai-voice-interconnector setup"* ]]
}

@test "reemplaza una instalación anterior" {
    mock_all

    # Pre-siembra una instalación anterior con contenido distinguible.
    old_app="$HOME/Applications/ai-voice-interconnector-arm64.app/Contents/MacOS"
    mkdir -p "$old_app"
    printf 'binario viejo' > "$old_app/ai-voice-interconnector"

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    # El binario fue reemplazado por el nuevo (script, no "binario viejo").
    new_bin="$HOME/Applications/ai-voice-interconnector-arm64.app/Contents/MacOS/ai-voice-interconnector"
    [ -x "$new_bin" ]
    ! grep -q "binario viejo" "$new_bin"
}
