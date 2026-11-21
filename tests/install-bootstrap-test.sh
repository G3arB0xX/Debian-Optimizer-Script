#!/usr/bin/env bash
# Host unit tests for debopty rename & zero-curl/wget bootstrap (no root/network)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FAILS=0
_fail() {
    echo "FAIL: $*" >&2
    FAILS=$((FAILS + 1))
}
_pass() { echo "OK: $*"; }

_assert_eq() {
    local got=$1 expected=$2 msg=$3
    if [[ "$got" != "$expected" ]]; then
        _fail "$msg (got='$got' expected='$expected')"
    else
        _pass "$msg"
    fi
}

_assert_file_contains() {
    local file=$1 pattern=$2 msg=$3
    if grep -qF -- "$pattern" "$file"; then
        _pass "$msg"
    else
        _fail "$msg (missing '$pattern' in $file)"
    fi
}

_assert_file_not_contains() {
    local file=$1 pattern=$2 msg=$3
    if grep -qF -- "$pattern" "$file"; then
        _fail "$msg (unexpected '$pattern' in $file)"
    else
        _pass "$msg"
    fi
}

TMP="$(mktemp -d /tmp/debopty-bootstrap-test.XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

# --- 1. Syntax & brand/hygiene checks ---
for f in deb_optimizer.sh install.sh scripts/common.sh scripts/network.sh scripts/tui.sh scripts/apps/xray.sh scripts/apps/easytier.sh scripts/apps/warp.sh templates/apps/xray/xray-rule-update.sh; do
    if bash -n "${ROOT}/${f}"; then
        _pass "bash -n ${f}"
    else
        _fail "bash -n ${f}"
    fi
done

_assert_file_contains "${ROOT}/deb_optimizer.sh" 'BIN_LINK="/usr/local/bin/debopty"' "deb_optimizer.sh sets BIN_LINK=/usr/local/bin/debopty"
_assert_file_contains "${ROOT}/deb_optimizer.sh" 'INSTALL_DIR="${INSTALL_DIR:-/opt/debopty}"' "deb_optimizer.sh defaults INSTALL_DIR=/opt/debopty"
_assert_file_contains "${ROOT}/deb_optimizer.sh" '"${INSTALL_DIR}/governance"' "deb_optimizer.sh strips governance in prod cleanup"
_assert_file_contains "${ROOT}/deb_optimizer.sh" '"${INSTALL_DIR}/rust"' "deb_optimizer.sh strips rust in prod cleanup"
_assert_file_contains "${ROOT}/deb_optimizer.sh" '_debopty_maybe_exec_rust "$@"' "deb_optimizer.sh invokes _debopty_maybe_exec_rust before self_install"
_assert_file_contains "${ROOT}/deb_optimizer.sh" "find \"\${INSTALL_DIR}\" -type f -name 'INDEX.md' -delete" "deb_optimizer.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${ROOT}/deb_optimizer.sh" 'VIBEINSTRCT.md' "deb_optimizer.sh removed VIBEINSTRCT.md typo"

_assert_file_contains "${ROOT}/install.sh" 'INSTALL_DIR="/opt/debopty"' "install.sh sets INSTALL_DIR=/opt/debopty"
_assert_file_contains "${ROOT}/install.sh" 'tools governance instructions tests rust' "install.sh strips governance and rust in prod cleanup"
_assert_file_contains "${ROOT}/install.sh" "find \"\$INSTALL_DIR\" -type f -name 'INDEX.md' -delete" "install.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${ROOT}/install.sh" 'VIBEINSTRCT.md' "install.sh has zero VIBEINSTRCT.md"

_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='governance'" "release.yml excludes governance from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='rust'" "release.yml excludes rust from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='INDEX.md'" "release.yml excludes INDEX.md from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "x86_64-unknown-linux-musl" "release.yml includes x86_64-unknown-linux-musl matrix target"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "aarch64-unknown-linux-musl" "release.yml includes aarch64-unknown-linux-musl matrix target"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "SHA256SUMS" "release.yml generates unified SHA256SUMS"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--features ratatui-ui" "release.yml builds release binaries with ratatui-ui feature"
_assert_file_contains "${ROOT}/tools/github-push.sh" "--exclude='governance'" "github-push.sh excludes governance from release tarball"
_assert_file_contains "${ROOT}/tools/github-push.sh" "--exclude='rust'" "github-push.sh excludes rust from release tarball"
_assert_file_contains "${ROOT}/tools/github-push.sh" "--exclude='INDEX.md'" "github-push.sh excludes INDEX.md from release tarball"
_assert_file_contains "${ROOT}/tools/github-push.sh" "instructions" "github-push.sh preserves FILTER_PATHS=( instructions )"

_assert_file_contains "${ROOT}/scripts/common.sh" 'REMOTE_REPO="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"' "common.sh REMOTE_REPO supports DEBOPTY_REPO"
_assert_file_contains "${ROOT}/scripts/common.sh" 'CONFIG_FILE="/etc/debopti/debopti.conf"' "common.sh keeps CONFIG_FILE backward-compatible"
_assert_file_contains "${ROOT}/scripts/tui.sh" 'ui_draw_header "debopty"' "tui.sh header renamed to debopty"
_assert_file_contains "${ROOT}/scripts/tui.sh" 'setup_tmux' "tui.sh wires setup_tmux"

# --- 2. Template existence & rendering ---
if [[ -f "${ROOT}/templates/common/debopty.sh" && -f "${ROOT}/templates/common/debopty.fish" ]]; then
    _pass "new debopty templates exist"
else
    _fail "missing templates/common/debopty.{sh,fish}"
fi

if [[ ! -e "${ROOT}/templates/common/debopti.sh" && ! -e "${ROOT}/templates/common/debopti.fish" ]]; then
    _pass "legacy debopti.{sh,fish} templates removed"
else
    _fail "legacy debopti.{sh,fish} templates still exist"
fi

(
    export SCRIPT_DIR="$ROOT"
    export INSTALL_DIR="/opt/debopty"
    export BIN_LINK="/usr/local/bin/debopty"
    export IS_CN_REGION="false"
    # shellcheck source=../scripts/common.sh
    source "${ROOT}/scripts/common.sh"
    render_template "templates/common/debopty.sh" "${TMP}/rendered_debopty.sh" "BIN_LINK=/usr/local/bin/debopty"
    render_template "templates/common/debopty.fish" "${TMP}/rendered_debopty.fish" "BIN_LINK=/usr/local/bin/debopty"
)
_assert_file_contains "${TMP}/rendered_debopty.sh" "alias debopty='/usr/local/bin/debopty'" "rendered debopty.sh alias"
_assert_file_not_contains "${TMP}/rendered_debopty.sh" "debopti" "rendered debopty.sh has no debopti"
_assert_file_contains "${TMP}/rendered_debopty.fish" "abbr -a debopty '/usr/local/bin/debopty'" "rendered debopty.fish abbr"
_assert_file_not_contains "${TMP}/rendered_debopty.fish" "debopti" "rendered debopty.fish has no debopti"

# --- 3. Source install.sh & test _debopty_bootstrap_fetch ---
export DEBOPTY_INSTALL_LIB_ONLY=1
# shellcheck source=../install.sh
source "${ROOT}/install.sh"
export DEBOPTY_NO_SUDO=1

_make_mock_bin() {
    local dir=$1
    mkdir -p "$dir"
    ln -sf "$(command -v rm)" "${dir}/rm"
}

# (a) curl-only
MOCK_A="${TMP}/mock_a"
_make_mock_bin "$MOCK_A"
cat > "${MOCK_A}/curl" <<'EOF'
#!/bin/bash
dest=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -o) dest=$2; shift 2 ;;
        *) shift ;;
    esac
done
printf 'from-curl\n' > "$dest"
EOF
chmod +x "${MOCK_A}/curl"
if PATH="$MOCK_A" _debopty_bootstrap_fetch "https://example.invalid/install.sh" "${TMP}/out_a"; then
    _assert_eq "$(cat "${TMP}/out_a")" "from-curl" "(a) curl-only bootstrap fetch"
else
    _fail "(a) curl-only bootstrap fetch failed"
fi

# (b) wget-only
MOCK_B="${TMP}/mock_b"
_make_mock_bin "$MOCK_B"
cat > "${MOCK_B}/wget" <<'EOF'
#!/bin/bash
dest=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -qO) dest=$2; shift 2 ;;
        *) shift ;;
    esac
done
printf 'from-wget\n' > "$dest"
EOF
chmod +x "${MOCK_B}/wget"
if PATH="$MOCK_B" _debopty_bootstrap_fetch "https://example.invalid/install.sh" "${TMP}/out_b"; then
    _assert_eq "$(cat "${TMP}/out_b")" "from-wget" "(b) wget-only bootstrap fetch"
else
    _fail "(b) wget-only bootstrap fetch failed"
fi

# (c) python3-only (real python3 + file:// URL if available, else stub)
MOCK_C="${TMP}/mock_c"
_make_mock_bin "$MOCK_C"
printf 'from-python3\n' > "${TMP}/fixture_py.txt"
if command -v python3 >/dev/null 2>&1; then
    ln -sf "$(command -v python3)" "${MOCK_C}/python3"
    py_url="file://${TMP}/fixture_py.txt"
else
    cat > "${MOCK_C}/python3" <<'EOF'
#!/bin/bash
printf 'from-python3\n' > "${@: -1}"
EOF
    chmod +x "${MOCK_C}/python3"
    py_url="https://example.invalid/install.sh"
fi
if PATH="$MOCK_C" _debopty_bootstrap_fetch "$py_url" "${TMP}/out_c"; then
    _assert_eq "$(cat "${TMP}/out_c")" "from-python3" "(c) python3-only bootstrap fetch"
else
    _fail "(c) python3-only bootstrap fetch failed"
fi

# (d) perl-only
MOCK_D="${TMP}/mock_d"
_make_mock_bin "$MOCK_D"
cat > "${MOCK_D}/perl" <<'EOF'
#!/bin/bash
if [[ "$*" != *"-MHTTP::Tiny"* ]]; then
    exit 2
fi
dest="${@: -1}"
printf 'from-perl\n' > "$dest"
EOF
chmod +x "${MOCK_D}/perl"
if PATH="$MOCK_D" _debopty_bootstrap_fetch "https://example.invalid/install.sh" "${TMP}/out_d"; then
    _assert_eq "$(cat "${TMP}/out_d")" "from-perl" "(d) perl-only bootstrap fetch"
else
    _fail "(d) perl-only bootstrap fetch failed"
fi

# (e) zero downloaders -> triggers mocked apt-get install ca-certificates curl -> curl
MOCK_E="${TMP}/mock_e"
_make_mock_bin "$MOCK_E"
APT_LOG_E="${TMP}/apt_e.log"
CHMOD_BIN="$(command -v chmod)"
cat > "${MOCK_E}/apt-get" <<EOF
#!/bin/bash
printf '%s\n' "\$*" >> "$APT_LOG_E"
if [[ "\$*" == *"install"* && "\$*" == *"ca-certificates"* && "\$*" == *"curl"* ]]; then
    printf '%s\n' '#!/bin/bash' 'dest=""' 'while [[ \$# -gt 0 ]]; do case "\$1" in -o) dest=\$2; shift 2;; *) shift;; esac; done' 'printf "from-apt-curl\n" > "\$dest"' > "${MOCK_E}/curl"
    "$CHMOD_BIN" +x "${MOCK_E}/curl"
fi
EOF
chmod +x "${MOCK_E}/apt-get"
if PATH="$MOCK_E" _debopty_bootstrap_fetch "https://example.invalid/install.sh" "${TMP}/out_e"; then
    _assert_eq "$(cat "${TMP}/out_e")" "from-apt-curl" "(e) zero-downloader apt-get fallback fetch"
    _assert_file_contains "$APT_LOG_E" "update -yq" "(e) apt-get update invoked"
    _assert_file_contains "$APT_LOG_E" "install -yq ca-certificates curl" "(e) apt-get install ca-certificates curl invoked"
else
    _fail "(e) zero-downloader apt-get fallback fetch failed"
fi

# (f) curl without ca-certificates (fails TLS) -> apt-get install ca-certificates curl -> succeeds
MOCK_F="${TMP}/mock_f"
_make_mock_bin "$MOCK_F"
APT_LOG_F="${TMP}/apt_f.log"
cat > "${MOCK_F}/curl" <<'EOF'
#!/bin/bash
exit 60
EOF
chmod +x "${MOCK_F}/curl"
cat > "${MOCK_F}/apt-get" <<EOF
#!/bin/bash
printf '%s\n' "\$*" >> "$APT_LOG_F"
if [[ "\$*" == *"install"* && "\$*" == *"ca-certificates"* && "\$*" == *"curl"* ]]; then
    printf '%s\n' '#!/bin/bash' 'dest=""' 'while [[ \$# -gt 0 ]]; do case "\$1" in -o) dest=\$2; shift 2;; *) shift;; esac; done' 'printf "from-fixed-ca-curl\n" > "\$dest"' > "${MOCK_F}/curl"
    "$CHMOD_BIN" +x "${MOCK_F}/curl"
fi
EOF
chmod +x "${MOCK_F}/apt-get"
if PATH="$MOCK_F" _debopty_bootstrap_fetch "https://example.invalid/install.sh" "${TMP}/out_f"; then
    _assert_eq "$(cat "${TMP}/out_f")" "from-fixed-ca-curl" "(f) missing ca-certificates recovery fetch"
    _assert_file_contains "$APT_LOG_F" "install -yq ca-certificates curl" "(f) apt-get install ca-certificates curl invoked"
else
    _fail "(f) missing ca-certificates recovery fetch failed"
fi

# --- 4. CN GitHub mirror regression & U6 Xray checks ---
DEAD_MIRRORS=(
    "ghproxy.homeboyc.cn"
    "hub.nuaa.cf"
    "raw.nuaa.cf"
    "kkgithub.com"
    "moeyy.cn/gh-proxy"
    "gitclone.com"
    "ghproxy.net"
    "github.akams.cn"
    "gh.llkk.cc"
    "hk.gh-proxy.org"
)
SCAN_TARGETS=(
    "${ROOT}/deb_optimizer.sh"
    "${ROOT}/install.sh"
    "${ROOT}/README.md"
    "${ROOT}/README_CN.md"
    "${ROOT}/scripts"
    "${ROOT}/templates"
    "${ROOT}/docs"
)
for dead in "${DEAD_MIRRORS[@]}"; do
    if grep -rnF "$dead" "${SCAN_TARGETS[@]}" >/dev/null 2>&1; then
        _fail "dead mirror '$dead' still present in repo"
    else
        _pass "dead mirror '$dead' removed"
    fi
done

_assert_file_contains "${ROOT}/README.md" 'https://gh-proxy.com/https://raw.githubusercontent.com/' "README.md CN one-liner uses gh-proxy.com"
_assert_file_contains "${ROOT}/README_CN.md" 'https://gh-proxy.com/https://raw.githubusercontent.com/' "README_CN.md CN one-liner uses gh-proxy.com"
_assert_file_contains "${ROOT}/scripts/apps/easytier.sh" 'https://gh-proxy.com/' "easytier.sh uses gh-proxy.com"
_assert_file_contains "${ROOT}/scripts/apps/warp.sh" 'https://gh-proxy.com/' "warp.sh API uses gh-proxy.com"
_assert_file_not_contains "${ROOT}/scripts/apps/warp.sh" 'https://ghfast.top/' "warp.sh does not use ghfast.top for API"

# U6 Xray assertions
_assert_file_contains "${ROOT}/scripts/apps/xray.sh" 'install --beta' "xray.sh installs pre-release (--beta) by default"
_assert_file_contains "${ROOT}/scripts/apps/xray.sh" '_xray_select_cn_mirror' "xray.sh has CN mirror selector"
_assert_file_contains "${ROOT}/scripts/apps/xray.sh" '_xray_rewrite_install_script_mirrors' "xray.sh has idempotent install-release.sh mirror rewriter"
_assert_file_contains "${ROOT}/docs/04-xray.md" 'install --beta' "docs/04-xray.md documents --beta"
_assert_file_contains "${ROOT}/docs/04-xray.md" 'gh-proxy.com' "docs/04-xray.md uses gh-proxy.com"

# Behavioral check: network.sh CN ranking & API safety (mocked curl)
(
    export SCRIPT_DIR="$ROOT"
    export IS_CN_REGION="true"
    # shellcheck source=../scripts/common.sh
    source "${ROOT}/scripts/common.sh"
    # shellcheck source=../scripts/network.sh
    source "${ROOT}/scripts/network.sh"

    MOCK_NET="${TMP}/mock_net"
    mkdir -p "$MOCK_NET"
    CURL_LOG="${TMP}/net_curl.log"
    cat > "${MOCK_NET}/curl" <<EOF
#!/bin/bash
url=""
dest=""
wfmt=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in
        -o) dest=\$2; shift 2 ;;
        -w) wfmt=\$2; shift 2 ;;
        -A|-H|--connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=\$1; shift ;;
    esac
done
printf '%s\n' "\$url" >> "$CURL_LOG"
if [[ "\$url" == *"api.github.com"* ]]; then
    printf '{"tag_name": "v1.2.3"}\n'
elif [[ -n "\$dest" ]]; then
    printf 'payload\n' > "\$dest"
    [[ -n "\$wfmt" ]] && printf '200'
fi
EOF
    chmod +x "${MOCK_NET}/curl"

    PATH="${MOCK_NET}:${PATH}" download_with_fallback "${TMP}/dl_out" "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh" >/dev/null 2>&1
    first_dl="$(head -n1 "$CURL_LOG")"
    if [[ "$first_dl" != "https://gh-proxy.com/https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh" ]]; then
        echo "unexpected first download mirror: $first_dl" >&2
        exit 1
    fi

    : > "$CURL_LOG"
    got_ver="$(PATH="${MOCK_NET}:${PATH}" get_latest_github_release "zhboner/realm")"
    if [[ "$got_ver" != "v1.2.3" ]]; then
        echo "unexpected release version: $got_ver" >&2
        exit 1
    fi
    first_api="$(head -n1 "$CURL_LOG")"
    if [[ "$first_api" != "https://gh-proxy.com/https://api.github.com/repos/zhboner/realm/releases/latest" ]]; then
        echo "unexpected first API mirror: $first_api" >&2
        exit 1
    fi
    if grep -q 'ghfast.top' "$CURL_LOG"; then
        echo "ghfast.top must not be called for API" >&2
        exit 1
    fi
) && _pass "network.sh CN mirror ranking & API safety behavioral test" || _fail "network.sh CN mirror ranking & API safety behavioral test"

# Behavioral check: xray.sh _xray_rewrite_install_script_mirrors idempotency & --beta url_zip preservation
(
    export SCRIPT_DIR="$ROOT"
    export IS_CN_REGION="true"
    # shellcheck source=../scripts/common.sh
    source "${ROOT}/scripts/common.sh"
    # shellcheck source=../scripts/apps/xray.sh
    source "${ROOT}/scripts/apps/xray.sh"

    cat > "${TMP}/xray-install-fixture.sh" <<'EOF'
local url='https://api.github.com/repos/XTLS/Xray-core/releases/latest'
url='https://api.github.com/repos/XTLS/Xray-core/releases'
url_zip="https://github.com/XTLS/Xray-core/releases/download/${releases_list[$i]}/Xray-linux-$MACHINE.zip"
local DOWNLOAD_LINK="https://github.com/XTLS/Xray-core/releases/download/${INSTALL_VERSION}/Xray-linux-${MACHINE}.zip"
local download_link_geoip="https://github.com/XTLS/Xray-rules-dat/releases/latest/download/${file_ip}"
local download_link_geosite="https://github.com/XTLS/Xray-rules-dat/releases/latest/download/${file_site}"
EOF
    _xray_rewrite_install_script_mirrors "${TMP}/xray-install-fixture.sh" "https://gh-proxy.com"
    _xray_rewrite_install_script_mirrors "${TMP}/xray-install-fixture.sh" "https://edgeone.gh-proxy.org/"
)
_assert_file_contains "${TMP}/xray-install-fixture.sh" 'https://edgeone.gh-proxy.org/https://api.github.com/repos/XTLS/Xray-core/releases/latest' "xray rewriter rewrites API URL cleanly"
_assert_file_contains "${TMP}/xray-install-fixture.sh" 'url_zip="https://github.com/XTLS/Xray-core/releases/download/' "xray rewriter preserves url_zip for --beta JSON match"
_assert_file_contains "${TMP}/xray-install-fixture.sh" 'DOWNLOAD_LINK="https://edgeone.gh-proxy.org/https://github.com/XTLS/Xray-core/releases/download/' "xray rewriter rewrites DOWNLOAD_LINK"
_assert_file_contains "${TMP}/xray-install-fixture.sh" 'https://edgeone.gh-proxy.org/https://github.com/XTLS/Xray-rules-dat/releases/latest/download/' "xray rewriter rewrites Xray-rules-dat"
_assert_file_not_contains "${TMP}/xray-install-fixture.sh" 'gh-proxy.com' "xray rewriter is idempotent without double-prefix"

# --- 5. Rust coexistence dispatch behavioral checks ---
COEX_FN="${TMP}/coex_fn.sh"
sed -n '/^_debopty_maybe_exec_rust()/,/^}/p' "${ROOT}/deb_optimizer.sh" > "$COEX_FN"
if [[ -s "$COEX_FN" ]]; then
    _pass "extracted _debopty_maybe_exec_rust from deb_optimizer.sh"
else
    _fail "failed to extract _debopty_maybe_exec_rust from deb_optimizer.sh"
fi

MOCK_RS_DIR="${TMP}/mock_rs_install"
mkdir -p "${MOCK_RS_DIR}/bin"
MOCK_RS_BIN="${TMP}/mock_debopty_rs"
cat > "$MOCK_RS_BIN" <<'EOF'
#!/bin/bash
printf 'rust-dispatched:%s\n' "$*"
EOF
chmod +x "$MOCK_RS_BIN"

cat > "${MOCK_RS_DIR}/bin/debopty" <<'EOF'
#!/bin/bash
printf 'install-bin-dispatched:%s\n' "$*"
EOF
chmod +x "${MOCK_RS_DIR}/bin/debopty"

coex_out="$(
    set -euo pipefail
    INSTALL_DIR="$MOCK_RS_DIR"
    warn() { printf 'WARN:%s\n' "$1" >&2; }
    # shellcheck source=/dev/null
    source "$COEX_FN"
    DEBOPTY_ENGINE=rust DEBOPTY_RS_BIN="$MOCK_RS_BIN" _debopty_maybe_exec_rust --flag val
    printf 'fell-through\n'
)"
_assert_eq "$coex_out" "rust-dispatched:--flag val" "DEBOPTY_ENGINE=rust DEBOPTY_RS_BIN dispatches via exec"

coex_install_out="$(
    set -euo pipefail
    INSTALL_DIR="$MOCK_RS_DIR"
    warn() { printf 'WARN:%s\n' "$1" >&2; }
    # shellcheck source=/dev/null
    source "$COEX_FN"
    DEBOPTY_ENGINE=rust DEBOPTY_RS_BIN="" _debopty_maybe_exec_rust status
    printf 'fell-through\n'
)"
_assert_eq "$coex_install_out" "install-bin-dispatched:status" "DEBOPTY_ENGINE=rust falls back to INSTALL_DIR/bin/debopty"

coex_loop_out="$(
    set -euo pipefail
    INSTALL_DIR="$MOCK_RS_DIR"
    warn() { printf 'WARN:%s\n' "$1" >&2; }
    # shellcheck source=/dev/null
    source "$COEX_FN"
    DEBOPTY_ENGINE=rust DEBOPTY_FROM_RUST=1 DEBOPTY_RS_BIN="$MOCK_RS_BIN" _debopty_maybe_exec_rust --flag
    printf 'blocked-recursion\n'
)"
_assert_eq "$coex_loop_out" "blocked-recursion" "DEBOPTY_FROM_RUST=1 blocks recursion"

coex_default_out="$(
    set -euo pipefail
    INSTALL_DIR="$MOCK_RS_DIR"
    warn() { printf 'WARN:%s\n' "$1" >&2; }
    # shellcheck source=/dev/null
    source "$COEX_FN"
    unset DEBOPTY_ENGINE || true
    DEBOPTY_RS_BIN="$MOCK_RS_BIN" _debopty_maybe_exec_rust
    printf 'default-bash\n'
)"
_assert_eq "$coex_default_out" "default-bash" "unset DEBOPTY_ENGINE keeps default Bash execution"

coex_missing_err="${TMP}/coex_missing.stderr"
coex_missing_out="$(
    set -euo pipefail
    INSTALL_DIR="${TMP}/missing_install_dir"
    warn() { printf 'WARN:%s\n' "$1" >&2; }
    # shellcheck source=/dev/null
    source "$COEX_FN"
    DEBOPTY_ENGINE=rust DEBOPTY_RS_BIN="${TMP}/nonexistent-rs-bin" _debopty_maybe_exec_rust 2>"$coex_missing_err"
    printf 'fallback-bash\n'
)"
_assert_eq "$coex_missing_out" "fallback-bash" "missing Rust binary falls back cleanly to Bash"
_assert_file_contains "$coex_missing_err" "DEBOPTY_ENGINE=rust" "missing Rust binary prints warning"

# --- 6. Binary-first install.sh helper checks (offline, no root) ---
_assert_eq "$(_debopty_detect_arch x86_64)" "amd64" "_debopty_detect_arch x86_64 -> amd64"
_assert_eq "$(_debopty_detect_arch amd64)" "amd64" "_debopty_detect_arch amd64 -> amd64"
_assert_eq "$(_debopty_detect_arch aarch64)" "arm64" "_debopty_detect_arch aarch64 -> arm64"
_assert_eq "$(_debopty_detect_arch arm64)" "arm64" "_debopty_detect_arch arm64 -> arm64"
if _debopty_detect_arch armv7l >/dev/null 2>&1; then
    _fail "_debopty_detect_arch armv7l should fail"
else
    _pass "_debopty_detect_arch armv7l rejected"
fi

BIN_FIXTURE_DIR="${TMP}/bin_rel_fixture"
mkdir -p "${BIN_FIXTURE_DIR}/pkg"
cat > "${BIN_FIXTURE_DIR}/pkg/debopty" <<'EOF'
#!/bin/bash
printf 'debopty-mock-bin:%s\n' "$*"
EOF
chmod 0755 "${BIN_FIXTURE_DIR}/pkg/debopty"
HOST_ARCH="$(_debopty_detect_arch)"
ASSET_NAME="debopty-v1.0.7-linux-${HOST_ARCH}.tar.gz"
tar -czf "${BIN_FIXTURE_DIR}/${ASSET_NAME}" -C "${BIN_FIXTURE_DIR}/pkg" debopty
(cd "$BIN_FIXTURE_DIR" && sha256sum "$ASSET_NAME" > SHA256SUMS)

if _debopty_verify_sha256 "${BIN_FIXTURE_DIR}/${ASSET_NAME}" "${BIN_FIXTURE_DIR}/SHA256SUMS" "$ASSET_NAME"; then
    _pass "_debopty_verify_sha256 passes on valid digest"
else
    _fail "_debopty_verify_sha256 failed on valid digest"
fi

printf '0000000000000000000000000000000000000000000000000000000000000000  %s\n' "$ASSET_NAME" > "${BIN_FIXTURE_DIR}/SHA256SUMS.bad"
if _debopty_verify_sha256 "${BIN_FIXTURE_DIR}/${ASSET_NAME}" "${BIN_FIXTURE_DIR}/SHA256SUMS.bad" "$ASSET_NAME"; then
    _fail "_debopty_verify_sha256 should reject mismatched digest"
else
    _pass "_debopty_verify_sha256 rejects mismatched digest"
fi

# Success path via local file:// base URL
INST_OK="${TMP}/inst_ok"
mkdir -p "$INST_OK"
cp "${ROOT}/VERSION" "${INST_OK}/VERSION"
RS_LINK_OK="${TMP}/debopty-rs-ok"
if DEBOPTY_RELEASE_BASE_URL="file://${BIN_FIXTURE_DIR}" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   DEBOPTY_RS_BIN_LINK="$RS_LINK_OK" \
   _debopty_install_rust_binary "$INST_OK" "false"; then
    if [[ -x "${INST_OK}/bin/debopty" && -L "$RS_LINK_OK" ]]; then
        _assert_eq "$("${INST_OK}/bin/debopty" version)" "debopty-mock-bin:version" "_debopty_install_rust_binary extracts executable binary and creates symlink"
    else
        _fail "_debopty_install_rust_binary missing bin/debopty or symlink"
    fi
else
    _fail "_debopty_install_rust_binary local file:// success path failed"
fi

# Checksum mismatch fallback leaves no binary
INST_BAD="${TMP}/inst_bad"
mkdir -p "$INST_BAD"
mkdir -p "${TMP}/bad_rel"
cp "${BIN_FIXTURE_DIR}/${ASSET_NAME}" "${TMP}/bad_rel/${ASSET_NAME}"
cp "${BIN_FIXTURE_DIR}/SHA256SUMS.bad" "${TMP}/bad_rel/SHA256SUMS"
if DEBOPTY_RELEASE_BASE_URL="file://${TMP}/bad_rel" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   DEBOPTY_RS_BIN_LINK="${TMP}/debopty-rs-bad" \
   _debopty_install_rust_binary "$INST_BAD" "false"; then
    _fail "_debopty_install_rust_binary should fail on SHA-256 mismatch"
else
    if [[ ! -e "${INST_BAD}/bin/debopty" ]]; then
        _pass "_debopty_install_rust_binary aborts cleanly on SHA-256 mismatch"
    else
        _fail "_debopty_install_rust_binary left binary behind after SHA-256 mismatch"
    fi
fi

# Unsupported arch fallback
MOCK_UNAME="${TMP}/mock_uname"
mkdir -p "$MOCK_UNAME"
cat > "${MOCK_UNAME}/uname" <<'EOF'
#!/bin/bash
echo "riscv64"
EOF
chmod +x "${MOCK_UNAME}/uname"
if PATH="${MOCK_UNAME}:${PATH}" \
   DEBOPTY_RELEASE_BASE_URL="file://${BIN_FIXTURE_DIR}" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   _debopty_install_rust_binary "$INST_BAD" "false"; then
    _fail "_debopty_install_rust_binary should fail on unsupported arch"
else
    _pass "_debopty_install_rust_binary rejects unsupported arch for graceful Bash fallback"
fi

# DEBOPTY_INSTALL_BINARY=0 skips download cleanly
INST_SKIP="${TMP}/inst_skip"
mkdir -p "$INST_SKIP"
if DEBOPTY_INSTALL_BINARY=0 \
   DEBOPTY_RELEASE_BASE_URL="file://${TMP}/missing_rel" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   _debopty_install_rust_binary "$INST_SKIP" "false"; then
    if [[ ! -e "${INST_SKIP}/bin/debopty" ]]; then
        _pass "DEBOPTY_INSTALL_BINARY=0 skips binary install cleanly"
    else
        _fail "DEBOPTY_INSTALL_BINARY=0 unexpectedly installed binary"
    fi
else
    _fail "DEBOPTY_INSTALL_BINARY=0 returned non-zero"
fi

# CN mirror pool test for _debopty_fetch_release_asset (no ghfast.top on API/latest)
MOCK_ASSET_CURL="${TMP}/mock_asset_curl"
mkdir -p "$MOCK_ASSET_CURL"
ln -sf "$(command -v rm)" "${MOCK_ASSET_CURL}/rm"
ASSET_CURL_LOG="${TMP}/asset_curl.log"
cat > "${MOCK_ASSET_CURL}/curl" <<EOF
#!/bin/bash
url=""
dest=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in
        -o) dest=\$2; shift 2 ;;
        --connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=\$1; shift ;;
    esac
done
printf '%s\n' "\$url" >> "$ASSET_CURL_LOG"
if [[ "\$url" == *"gh-proxy.com"* ]]; then
    printf 'ok\n' > "\$dest"
    exit 0
fi
exit 22
EOF
chmod +x "${MOCK_ASSET_CURL}/curl"
if PATH="${MOCK_ASSET_CURL}" _debopty_fetch_release_asset "true" "https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/download/v1.0.7/SHA256SUMS" "${TMP}/asset_out"; then
    _assert_eq "$(head -n1 "$ASSET_CURL_LOG")" "https://gh-proxy.com/https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/download/v1.0.7/SHA256SUMS" "_debopty_fetch_release_asset CN uses gh-proxy.com first"
else
    _fail "_debopty_fetch_release_asset CN failed"
fi

: > "$ASSET_CURL_LOG"
cat > "${MOCK_ASSET_CURL}/curl" <<EOF
#!/bin/bash
url=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in
        -o|--connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=\$1; shift ;;
    esac
done
printf '%s\n' "\$url" >> "$ASSET_CURL_LOG"
exit 22
EOF
chmod +x "${MOCK_ASSET_CURL}/curl"
PATH="${MOCK_ASSET_CURL}" _debopty_fetch_release_asset "true" "https://api.github.com/repos/G3arB0xX/Debian-Optimizer-Script/releases/latest" "${TMP}/api_out" || true
if grep -qF 'ghfast.top' "$ASSET_CURL_LOG"; then
    _fail "_debopty_fetch_release_asset must skip ghfast.top for API URLs"
else
    _pass "_debopty_fetch_release_asset skips ghfast.top for API URLs"
fi

if [[ "$FAILS" -ne 0 ]]; then
    echo "install-bootstrap-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "install-bootstrap-test: all passed"
