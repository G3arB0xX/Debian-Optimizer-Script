#!/bin/bash
# DevOps 模块（共享 CLI 依赖 / Fish / Micro / Yazi）容器内验证
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
export CI=true

for _i in $(seq 1 30); do
    if ! fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

DEVOPS_TEST_MODE="${DEVOPS_TEST_MODE:-fast}"

_fail() {
    echo "ASSERT_FAIL: $*"
    exit 1
}

_assert_cmd() {
    command -v "$1" >/dev/null 2>&1 || _fail "missing command: $1"
}

_assert_fzf_min() {
    local ver min="0.53"
    ver=$(fzf --version 2>/dev/null | awk '{print $1}')
    [[ -n "$ver" ]] || _fail "fzf version empty"
    if ! printf '%s\n%s\n' "$min" "$ver" | sort -C -V 2>/dev/null; then
        if [[ "$(printf '%s\n%s\n' "$min" "$ver" | sort -V | head -1)" != "$min" ]]; then
            _fail "fzf ${ver} < ${min}"
        fi
    fi
}

_assert_shared_cli_deps() {
    _assert_cmd fzf
    _assert_fzf_min
    _assert_cmd fd
    _assert_cmd rg
    _assert_cmd zoxide
    _assert_cmd bat
    if ! command -v xclip >/dev/null 2>&1 && ! command -v wl-copy >/dev/null 2>&1 && ! command -v xsel >/dev/null 2>&1; then
        _fail "no clipboard tool (xclip/wl-copy/xsel)"
    fi
    if ! fc-list 2>/dev/null | grep -qi "0xProtoNerdFont"; then
        if [[ ! -f /usr/local/share/fonts/0xProto-Nerd-Font/.debopti-installed ]]; then
            _fail "0xProto Nerd Font not installed"
        fi
    fi
}

_setup_test_env() {
    apt-get update -yq >/dev/null 2>&1
    apt-get install -yq --no-install-recommends \
        ca-certificates curl git sudo fontconfig >/dev/null 2>&1

    if ! id testuser >/dev/null 2>&1; then
        useradd -m -s /bin/bash testuser
    fi

    mkdir -p /etc/debopti /opt/debopti/.cursor
    cat > /etc/debopti/debopti.conf << 'EOF'
IS_CN_REGION="false"
BASE_OPTIMIZED="false"
INSTALLED="false"
SSH_HARDENED="false"
EDITOR_CMD="micro"
PODMAN_GC_ENABLED="false"
YAZI_MEDIA_PREVIEW="false"
EOF

    export SCRIPT_DIR=/opt/debopti
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/common.sh
    load_project_config
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/network.sh
    # shellcheck disable=SC1091
    source /opt/debopti/scripts/apps/devops.sh

    bash -n /opt/debopti/scripts/apps/devops.sh
}

echo "=== SETUP (${DEBIAN_TAG:-unknown}) [${DEVOPS_TEST_MODE}] ==="
_setup_test_env

echo "=== TEST: Yazi first (shared deps + binary, media=false) ==="
if ! install_yazi; then
    echo "INSTALL_YAZI_FAILED"
    exit 1
fi
_assert_cmd yazi
_assert_cmd ya
_assert_shared_cli_deps
test -f /usr/local/bin/yazi
test -f /etc/profile.d/yazi_wrapper.sh

echo "=== TEST: Micro after Yazi (idempotent shared deps) ==="
if ! install_micro; then
    echo "INSTALL_MICRO_FAILED"
    exit 1
fi
_assert_cmd micro
_assert_shared_cli_deps
test -f /etc/profile.d/micro_env.sh || test -d "$(eval echo ~testuser)/.config/micro"
grep -q 'goos\.MkdirAll(sessionsPath, goos\.ModePerm)' \
    "$(eval echo ~testuser)/.config/micro/plug/MicroOmni/Session.lua" 2>/dev/null \
    || _fail "MicroOmni Session.lua missing goos.MkdirAll compat patch"

echo "=== TEST: Fish after Micro/Yazi (idempotent shared deps) ==="
if ! install_fish; then
    echo "INSTALL_FISH_FAILED"
    exit 1
fi
_assert_cmd fish
_assert_shared_cli_deps
test -d "$(eval echo ~testuser)/.config/fish"
test -f "$(eval echo ~testuser)/.config/fish/functions/_puffer_fish_expand_dot.fish" \
    || _fail "missing puffer-fish Fish 3.x compat override"

echo "=== TEST: Yazi reinstall (idempotent) ==="
if ! install_yazi; then
    echo "REINSTALL_YAZI_FAILED"
    exit 1
fi
_assert_shared_cli_deps

if [[ "$DEVOPS_TEST_MODE" == "full" ]]; then
    echo "=== TEST: Yazi media preview bundle ==="
    save_project_config "YAZI_MEDIA_PREVIEW" "true"
    # shellcheck disable=SC1091
    source /etc/debopti/debopti.conf
    _install_yazi_dependencies
    _assert_cmd ffmpeg
    command -v pdftoppm >/dev/null 2>&1 || _fail "missing pdftoppm (poppler-utils)"
    command -v convert >/dev/null 2>&1 || _fail "missing convert (imagemagick)"
    if [[ "$(uname -m)" == "x86_64" ]]; then
        _assert_cmd resvg
    fi

    echo "=== TEST: shared deps direct call (idempotent) ==="
    _install_devops_shared_cli_deps
    _assert_shared_cli_deps

    echo "=== TEST: fd symlink stable ==="
    fd_path=$(command -v fd)
    [[ -x "$fd_path" ]] || _fail "fd not executable: $fd_path"
    fd --version >/dev/null 2>&1 || true

    echo "=== TEST: bat via batcat symlink ==="
    bat --version >/dev/null 2>&1 || _fail "bat not working"
fi

echo "ALL_TESTS_PASSED"
