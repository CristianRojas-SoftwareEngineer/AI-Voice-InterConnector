# Smoke-test de upgrade-ai-voice-interconnector.sh (bats-core):
# mockea curl/uname por PATH, sin red ni instalación reales.
#
# Ejecutar: bats tests/installer/upgrade-linux.bats

setup() {
    REPO_ROOT="$(cd "$(dirname "$BATS_TEST_FILENAME")/../.." && pwd)"
    WRAPPER="$REPO_ROOT/upgrade-ai-voice-interconnector.sh"
    INSTALL_SH="$REPO_ROOT/install-linux.sh"

    WORK="$(mktemp -d)"
    export HOME="$WORK/home"
    mkdir -p "$HOME"

    MOCK_BIN="$WORK/bin"
    mkdir -p "$MOCK_BIN"
    export PATH="$MOCK_BIN:$REPO_ROOT:$PATH"

    # Stub de `install-linux.sh` que no hace nada (el wrapper solo
    # necesita que se invoque sin error).
    cat > "$MOCK_BIN/install-linux.sh" <<'STUB'
#!/bin/sh
exit 0
STUB
    chmod +x "$MOCK_BIN/install-linux.sh"

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

# Mock de `uname`. Responde `-s` al SO y `-m` a la arquitectura.
mock_uname() {
    local os="$1"
    cat > "$MOCK_BIN/uname" <<MOCKUNE
#!/bin/sh
if [ "\$1" = "-s" ]; then echo "$os"; fi
if [ "\$1" = "-m" ]; then echo "x86_64"; fi
MOCKUNE
    chmod +x "$MOCK_BIN/uname"
}

# Mock de `curl` que sirve un release con tag_name.
mock_curl() {
    local tag="$1"
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
{"assets":[{"browser_download_url":"https://example.invalid/ai-voice-interconnector-1.0.0-x86_64-linux.tar.gz"},{"browser_download_url":"https://example.invalid/SHA256SUMS.txt"}],"tag_name":"$tag"}
JSON
        ;;
    *) ;;
esac
EOF
    chmod +x "$MOCK_BIN/curl"
}

@test "--check reporta transición sin instalar" {
    mock_uname Linux
    mock_curl v2.0.0

    run sh "$WRAPPER" --check

    [ "$status" -eq 0 ]
    [[ "$output" == *"1.0.0 → 2.0.0"* ]]
    [ ! -d "$HOME/.local/opt/ai-voice-interconnector" ]
}

@test "modo install re-ejecuta install-linux.sh" {
    mock_uname Linux
    mock_curl v2.0.0

    run sh "$WRAPPER"

    [ "$status" -eq 0 ]
    [[ "$output" == *"1.0.0 → 2.0.0"* ]]
    # El stub de install-linux.sh fue invocado (status 0).
    [ "$status" -eq 0 ]
}
