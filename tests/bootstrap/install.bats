# Suite del bootstrap POSIX packaging/bootstrap/install.sh (bats-core).
#
# Corre contra el servidor falso local (support/serve.py, HTTPS con CA de
# prueba) con assets por target, SHA256SUMS.txt coherente o corrupto y raíces
# reubicadas a temporales. Cubre los criterios 3 (checksum con staging
# borrado), 4 (plataforma no soportada antes de descargar), 5 (binario
# incompatible con diagnóstico) y 10 (paso de opciones y confirmación con
# terminal). Sin casos --check (decisión d) y sin red real.
#
# Ejecutar: bats tests/bootstrap/install.bats (con `sh`, que es dash en WSL).

load "support/setup"

setup() {
    REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
    INSTALL_SH="$REPO_ROOT/packaging/bootstrap/install.sh"
    SUPPORT="$REPO_ROOT/tests/bootstrap/support"

    WORK="$(mktemp -d)"
    export HOME="$WORK/home"
    mkdir -p "$HOME"

    # La primera prueba fija la ruta base; las siguientes reutilizan la misma
    # para no acumular directorios ya borrados en el PATH.
    if [ -z "${HARNESS_BASE_PATH:-}" ]; then
        HARNESS_BASE_PATH="$PATH"
        export HARNESS_BASE_PATH
    fi
    MOCK_BIN="$WORK/mock-bin"
    mkdir -p "$MOCK_BIN"
    export PATH="$MOCK_BIN:$HARNESS_BASE_PATH"

    SERVE_DIR="$WORK/serve"
    mkdir -p "$SERVE_DIR"
    harness_make_assets "$SERVE_DIR"
    harness_write_sums "$SERVE_DIR" ok
    sh "$SUPPORT/make-cert.sh" "$WORK/cert"
    harness_start_server "$SERVE_DIR" "$WORK/cert"
    harness_relocate "$WORK/roots"

    unset AVI_VERSION AVI_NO_SETUP AVI_NO_MODIFY_PATH AVI_YES AVI_FAKE_MODE
}

teardown() {
    harness_stop_server
    rm -rf "$WORK"
}

# No queda ningún staging hermano tras la prueba.
assert_no_staging_left() {
    leftovers="$(find "$WORK/roots" -maxdepth 1 -name '.ai-voice-interconnector-staging-*')"
    [ -z "$leftovers" ]
}

@test "instala el asset x86_64-linux cuando uname -m es x86_64" {
    mock_uname x86_64

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz"* ]]
    grep -q "GET /v9.9.9/ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz" "$SERVER_LOG"
    assert_no_staging_left
}

@test "instala el asset arm64-linux cuando uname -m es aarch64" {
    mock_uname aarch64

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-9.9.9-arm64-linux.tar.gz"* ]]
    assert_no_staging_left
}

@test "en macOS con Rosetta manda sysctl y elige arm64-macos" {
    mock_uname x86_64 Darwin
    mock_sysctl 1

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    [[ "$output" == *"ai-voice-interconnector-9.9.9-arm64-macos.tar.gz"* ]]
    assert_no_staging_left
}

@test "Mac Intel (sysctl sin ARM64) falla como plataforma no soportada" {
    mock_uname x86_64 Darwin
    mock_sysctl 0
    export AVI_DOWNLOAD_BASE_URL="http://127.0.0.1:1"

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"unsupported_platform"* ]]
}

@test "arquitectura no soportada falla antes de descargar" {
    mock_uname riscv64
    # Puerto cerrado: si intentara descargar, el error sería de red, no de plataforma.
    export AVI_DOWNLOAD_BASE_URL="http://127.0.0.1:1"

    run sh "$INSTALL_SH" --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"unsupported_platform"* ]]
}

@test "la opción --version manda sobre AVI_VERSION" {
    mock_uname x86_64
    export AVI_VERSION="8.8.8"

    # El arnés solo sirve 9.9.9: si eligiera 8.8.8, la descarga fallaría.
    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
}

@test "AVI_VERSION se usa sin --version" {
    mock_uname x86_64
    export AVI_VERSION="9.9.9"

    run sh "$INSTALL_SH" --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    [[ "$(cat "$AVI_FAKE_LOG")" == "self install --no-setup --no-modify-path --yes" ]]
}

@test "la versión estampada se usa sin --version ni AVI_VERSION" {
    mock_uname x86_64
    cp "$INSTALL_SH" "$WORK/install-stamped.sh"
    sed -i "s/__AVI_STAMPED_VERSION__/9.9.9/" "$WORK/install-stamped.sh"

    run sh "$WORK/install-stamped.sh" --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    grep -q "GET /v9.9.9/ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz" "$SERVER_LOG"
}

@test "versión inválida falla sin descargar" {
    mock_uname x86_64
    export AVI_DOWNLOAD_BASE_URL="http://127.0.0.1:1"

    run sh "$INSTALL_SH" --version "abc" --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"usage_error"* ]]
}

@test "checksum corrupto falla con staging borrado y nada instalado (criterio 3)" {
    mock_uname x86_64
    harness_write_sums "$SERVE_DIR" corrupt "ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz"

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum_mismatch"* ]]
    assert_no_staging_left
    [ ! -e "$AVI_INSTALL_DIR/ai-voice-interconnector" ]
}

@test "SHA256SUMS sin línea para el asset falla como checksum inválido" {
    mock_uname x86_64
    harness_write_sums "$SERVE_DIR" missing "ai-voice-interconnector-9.9.9-x86_64-linux.tar.gz"

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"checksum_mismatch"* ]]
    assert_no_staging_left
}

@test "binario incompatible diagnostica glibc y deja lo instalado intacto (criterio 5)" {
    mock_uname x86_64
    export AVI_FAKE_MODE="noexec"
    printf 'instalación previa' > "$AVI_INSTALL_DIR/sentinel"

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -ne 0 ]
    [[ "$output" == *"binary_incompatible"* ]]
    [[ "$output" == *"glibc"* ]]
    [[ "$output" == *"BUILD.md"* ]]
    [ "$(cat "$AVI_INSTALL_DIR/sentinel")" = "instalación previa" ]
    assert_no_staging_left
}

@test "las opciones se pasan tal cual a self install (criterio 10)" {
    mock_uname x86_64

    run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path --yes

    [ "$status" -eq 0 ]
    [ "$(cat "$AVI_FAKE_LOG")" = "self install --no-setup --no-modify-path --yes" ]
}

@test "las variables AVI_* equivalen a las opciones" {
    mock_uname x86_64
    export AVI_VERSION="9.9.9"
    export AVI_NO_SETUP="1"
    export AVI_NO_MODIFY_PATH="true"
    export AVI_YES="yes"

    run sh "$INSTALL_SH"

    [ "$status" -eq 0 ]
    [ "$(cat "$AVI_FAKE_LOG")" = "self install --no-setup --no-modify-path --yes" ]
}

@test "sin terminal y sin --yes la confirmación no procede" {
    mock_uname x86_64
    # setsid retira la terminal rectora: sin ella /dev/tty no existe y el
    # bootstrap no puede redirigir la confirmación (donde hay rectora, como
    # una consola local, la redirección procede y este caso no aplica).
    if command -v setsid >/dev/null 2>&1; then
        run setsid --wait sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path < /dev/null
    elif [ -e /dev/tty ]; then
        skip "sin setsid y con terminal rectora no se puede aislar la falta de terminal"
    else
        run sh "$INSTALL_SH" --version 9.9.9 --no-setup --no-modify-path < /dev/null
    fi

    [ "$status" -ne 0 ]
    [[ "$output" == *"confirmación"* ]]
}

@test "con terminal la confirmación procede sin --yes (criterio 10)" {
    if [ "$(uname -s)" != "Linux" ] || ! command -v script >/dev/null 2>&1; then
        skip "requiere script(1) de util-linux"
    fi
    mock_uname x86_64

    # stdin por tubería pero con terminal rectora: el bootstrap redirige
    # /dev/tty al binario y el doble, que exige terminal sin --yes, procede.
    run script -qec "sh '$INSTALL_SH' --version 9.9.9 --no-setup --no-modify-path < /dev/null" /dev/null

    [ "$status" -eq 0 ]
    [ "$(cat "$AVI_FAKE_LOG")" = "self install --no-setup --no-modify-path" ]
    assert_no_staging_left
}
