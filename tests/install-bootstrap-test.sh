#!/usr/bin/env bash
# Host unit tests for debopty rename & zero-curl/wget bootstrap (no root/network)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
LEGACY_ROOT="$ROOT"
[[ -f "${LEGACY_ROOT}/deb_optimizer.sh" ]] || LEGACY_ROOT="${ROOT}/archive/legacy-bash"
LEGACY_SCRIPTS="${LEGACY_ROOT}/scripts"

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
for f in "${LEGACY_ROOT}/deb_optimizer.sh" "${ROOT}/install.sh" "${LEGACY_SCRIPTS}/common.sh" "${LEGACY_SCRIPTS}/network.sh" "${LEGACY_SCRIPTS}/tui.sh" "${LEGACY_SCRIPTS}/apps/xray.sh" "${LEGACY_SCRIPTS}/apps/easytier.sh" "${LEGACY_SCRIPTS}/apps/warp.sh" "${ROOT}/templates/apps/xray/xray-rule-update.sh"; do
    if bash -n "$f"; then
        _pass "bash -n ${f#"${ROOT}"/}"
    else
        _fail "bash -n ${f#"${ROOT}"/}"
    fi
done

_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" 'BIN_LINK="/usr/local/bin/debopty"' "deb_optimizer.sh sets BIN_LINK=/usr/local/bin/debopty"
_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" 'INSTALL_DIR="${INSTALL_DIR:-/opt/debopty}"' "deb_optimizer.sh defaults INSTALL_DIR=/opt/debopty"
_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" '"${INSTALL_DIR}/governance"' "deb_optimizer.sh strips governance in prod cleanup"
_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" '"${INSTALL_DIR}/rust"' "deb_optimizer.sh strips rust in prod cleanup"
_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" '_debopty_maybe_exec_rust "$@"' "deb_optimizer.sh invokes _debopty_maybe_exec_rust before self_install"
_assert_file_contains "${LEGACY_ROOT}/deb_optimizer.sh" "find \"\${INSTALL_DIR}\" -type f -name 'INDEX.md' -delete" "deb_optimizer.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${LEGACY_ROOT}/deb_optimizer.sh" 'VIBEINSTRCT.md' "deb_optimizer.sh removed VIBEINSTRCT.md typo"

_assert_file_contains "${ROOT}/install.sh" 'INSTALL_DIR="/opt/debopty"' "install.sh sets INSTALL_DIR=/opt/debopty"
_assert_file_contains "${ROOT}/install.sh" '.agents tools governance instructions tests rust archive' "install.sh strips .agents, governance, rust and archive in prod cleanup"
_assert_file_contains "${ROOT}/install.sh" 'DEBOPTY_BIN_LINK:-/usr/local/bin/debopty' "install.sh links primary /usr/local/bin/debopty to Rust binary"
_assert_file_contains "${ROOT}/install.sh" 'DEBOPTY_F2B_BIN_LINK:-/usr/local/bin/debopty-f2b-sync' "install.sh links multicall /usr/local/bin/debopty-f2b-sync"
_assert_file_contains "${ROOT}/install.sh" 'exec "${INSTALL_DIR}/bin/debopty" "$@"' "install.sh defaults to executing Rust binary"
_assert_file_contains "${ROOT}/install.sh" "find \"\$INSTALL_DIR\" -type f -name 'INDEX.md' -delete" "install.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${ROOT}/install.sh" 'VIBEINSTRCT.md' "install.sh has zero VIBEINSTRCT.md"

_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='.agents'" "release.yml excludes .agents from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='governance'" "release.yml excludes governance from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='rust'" "release.yml excludes rust from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='archive'" "release.yml excludes archive from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='INDEX.md'" "release.yml excludes INDEX.md from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "x86_64-unknown-linux-musl" "release.yml includes x86_64-unknown-linux-musl matrix target"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "aarch64-unknown-linux-musl" "release.yml includes aarch64-unknown-linux-musl matrix target"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "SHA256SUMS" "release.yml generates unified SHA256SUMS"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--features ratatui-ui" "release.yml builds release binaries with ratatui-ui feature"
_assert_file_not_contains "${ROOT}/tools/github-push.sh" "gh release create" "github-push.sh delegates GitHub Release creation exclusively to release.yml"
_assert_file_contains "${ROOT}/tools/github-push.sh" "--scan-git-history" "github-push.sh enforces full-history secret scan via verify_repo_hygiene.py"
_assert_file_contains "${ROOT}/tools/github-push.sh" "git-filter-repo" "github-push.sh rewrites history with git-filter-repo"
_assert_file_contains "${ROOT}/tools/github-push.sh" "_sync_local_tag" "github-push.sh syncs pushed release tag back to local workspace"
_assert_file_contains "${ROOT}/tools/github-push.sh" "_setup_gh_plugin_auth" "github-push.sh prefers GitHub plugin (gh/gh-helper) auth"

_assert_file_contains "${LEGACY_SCRIPTS}/common.sh" 'REMOTE_REPO="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"' "common.sh REMOTE_REPO supports DEBOPTY_REPO"
_assert_file_contains "${LEGACY_SCRIPTS}/common.sh" 'CONFIG_FILE="/etc/debopti/debopti.conf"' "common.sh keeps CONFIG_FILE backward-compatible"
_assert_file_contains "${LEGACY_SCRIPTS}/tui.sh" 'ui_draw_header "debopty"' "tui.sh header renamed to debopty"
_assert_file_contains "${LEGACY_SCRIPTS}/tui.sh" 'setup_tmux' "tui.sh wires setup_tmux"

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
    # shellcheck source=../archive/legacy-bash/scripts/common.sh
    source "${LEGACY_SCRIPTS}/common.sh"
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
    "${LEGACY_ROOT}/deb_optimizer.sh"
    "${ROOT}/install.sh"
    "${ROOT}/README.md"
    "${ROOT}/README_CN.md"
    "${LEGACY_SCRIPTS}"
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
_assert_file_contains "${LEGACY_SCRIPTS}/apps/easytier.sh" 'https://gh-proxy.com/' "easytier.sh uses gh-proxy.com"
_assert_file_contains "${LEGACY_SCRIPTS}/apps/warp.sh" 'https://gh-proxy.com/' "warp.sh API uses gh-proxy.com"
_assert_file_not_contains "${LEGACY_SCRIPTS}/apps/warp.sh" 'https://ghfast.top/' "warp.sh does not use ghfast.top for API"

# U6 Xray assertions
_assert_file_contains "${LEGACY_SCRIPTS}/apps/xray.sh" 'install --beta' "xray.sh installs pre-release (--beta) by default"
_assert_file_contains "${LEGACY_SCRIPTS}/apps/xray.sh" '_xray_select_cn_mirror' "xray.sh has CN mirror selector"
_assert_file_contains "${LEGACY_SCRIPTS}/apps/xray.sh" '_xray_rewrite_install_script_mirrors' "xray.sh has idempotent install-release.sh mirror rewriter"
_assert_file_contains "${ROOT}/docs/04-xray.md" 'install --beta' "docs/04-xray.md documents --beta"
_assert_file_contains "${ROOT}/docs/04-xray.md" 'gh-proxy.com' "docs/04-xray.md uses gh-proxy.com"

# Behavioral check: network.sh CN ranking & API safety (mocked curl)
(
    export SCRIPT_DIR="$ROOT"
    export IS_CN_REGION="true"
    # shellcheck source=../archive/legacy-bash/scripts/common.sh
    source "${LEGACY_SCRIPTS}/common.sh"
    # shellcheck source=../archive/legacy-bash/scripts/network.sh
    source "${LEGACY_SCRIPTS}/network.sh"

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
    # shellcheck source=../archive/legacy-bash/scripts/common.sh
    source "${LEGACY_SCRIPTS}/common.sh"
    # shellcheck source=../archive/legacy-bash/scripts/apps/xray.sh
    source "${LEGACY_SCRIPTS}/apps/xray.sh"

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
sed -n '/^_debopty_maybe_exec_rust()/,/^}/p' "${LEGACY_ROOT}/deb_optimizer.sh" > "$COEX_FN"
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
BIN_LINK_OK="${TMP}/debopty-primary-ok"
F2B_LINK_OK="${TMP}/debopty-f2b-sync-ok"
RS_LINK_OK="${TMP}/debopty-rs-ok"
PROFILE_OK="${TMP}/profile.d"
FISH_OK="${TMP}/fish.conf.d"
LEGACY_LINK_OK="${TMP}/debopti-legacy"
ln -sf "/opt/debopti/deb_optimizer.sh" "$LEGACY_LINK_OK"
mkdir -p "$PROFILE_OK" "$FISH_OK"
printf 'alias debopti=old\n' > "${PROFILE_OK}/debopti.sh"
if DEBOPTY_RELEASE_BASE_URL="file://${BIN_FIXTURE_DIR}" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   DEBOPTY_BIN_LINK="$BIN_LINK_OK" \
   DEBOPTY_F2B_BIN_LINK="$F2B_LINK_OK" \
   DEBOPTY_RS_BIN_LINK="$RS_LINK_OK" \
   DEBOPTY_LEGACY_BIN_LINK="$LEGACY_LINK_OK" \
   DEBOPTY_PROFILE_DIR="$PROFILE_OK" \
   DEBOPTY_FISH_CONF_DIR="$FISH_OK" \
   _debopty_install_rust_binary "$INST_OK" "false"; then
    if [[ -x "${INST_OK}/bin/debopty" && -L "$BIN_LINK_OK" && -L "$F2B_LINK_OK" && -L "$RS_LINK_OK" && ! -e "$LEGACY_LINK_OK" && ! -e "${PROFILE_OK}/debopti.sh" ]]; then
        _assert_eq "$("${INST_OK}/bin/debopty" version)" "debopty-mock-bin:version" "_debopty_install_rust_binary extracts executable binary, links primary+multicall+compat, and removes legacy debopti"
    else
        _fail "_debopty_install_rust_binary missing bin/debopty, primary/f2b/rs symlinks, or failed legacy cleanup"
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
        -A|-H|--user-agent|--connect-timeout|--max-time) shift 2 ;;
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
        -o|-A|-H|--user-agent|--connect-timeout|--max-time) shift 2 ;;
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

# --- 7. U6 Bootstrap arch, SHA-256 fallback & binary-first degradation checks ---
for bad_arch in riscv64 i686 s390x ""; do
    if [[ -z "$bad_arch" ]]; then
        MOCK_EMPTY_UNAME="${TMP}/mock_empty_uname"
        mkdir -p "$MOCK_EMPTY_UNAME"
        printf '#!/bin/bash\nprintf ""\n' > "${MOCK_EMPTY_UNAME}/uname"
        chmod +x "${MOCK_EMPTY_UNAME}/uname"
        if PATH="${MOCK_EMPTY_UNAME}:${PATH}" _debopty_detect_arch >/dev/null 2>&1; then
            _fail "_debopty_detect_arch empty uname should fail"
        else
            _pass "_debopty_detect_arch empty uname rejected"
        fi
    else
        if _debopty_detect_arch "$bad_arch" >/dev/null 2>&1; then
            _fail "_debopty_detect_arch ${bad_arch} should fail"
        else
            _pass "_debopty_detect_arch ${bad_arch} rejected"
        fi
    fi
done

U6_SHA_DIR="${TMP}/u6_sha"
mkdir -p "$U6_SHA_DIR"
printf 'debopty-u6-payload-bytes\n' > "${U6_SHA_DIR}/asset.tar.gz"
VALID_HEX="$(sha256sum "${U6_SHA_DIR}/asset.tar.gz" | awk '{print $1}')"
UPPER_HEX="${VALID_HEX^^}"

# Uppercase hex, binary '*' prefix, and './' prefix all pass
printf '%s  asset.tar.gz\n' "$UPPER_HEX" > "${U6_SHA_DIR}/sums_upper"
printf '%s *asset.tar.gz\n' "$VALID_HEX" > "${U6_SHA_DIR}/sums_star"
printf '%s  ./asset.tar.gz\n' "$VALID_HEX" > "${U6_SHA_DIR}/sums_dotslash"
for variant in sums_upper sums_star sums_dotslash; do
    if _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/${variant}" "asset.tar.gz"; then
        _pass "_debopty_verify_sha256 accepts ${variant}"
    else
        _fail "_debopty_verify_sha256 rejected ${variant}"
    fi
done

# Missing asset, corrupt/short hex, and empty sums file all fail closed
printf '%s  other-asset.tar.gz\n' "$VALID_HEX" > "${U6_SHA_DIR}/sums_missing"
printf 'not-a-valid-hex-digest  asset.tar.gz\n' > "${U6_SHA_DIR}/sums_corrupt"
: > "${U6_SHA_DIR}/sums_empty"
for bad_variant in sums_missing sums_corrupt sums_empty; do
    if _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/${bad_variant}" "asset.tar.gz"; then
        _fail "_debopty_verify_sha256 should reject ${bad_variant}"
    else
        _pass "_debopty_verify_sha256 rejects ${bad_variant}"
    fi
done

# openssl fallback when sha256sum is absent
if command -v openssl >/dev/null 2>&1; then
    MOCK_OSSL="${TMP}/mock_ossl"
    mkdir -p "$MOCK_OSSL"
    ln -sf "$(command -v openssl)" "${MOCK_OSSL}/openssl"
    if PATH="$MOCK_OSSL" _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/sums_star" "asset.tar.gz"; then
        _pass "_debopty_verify_sha256 falls back to openssl when sha256sum absent"
    else
        _fail "_debopty_verify_sha256 openssl fallback failed"
    fi
fi

# python3 fallback when sha256sum and openssl are both absent
if command -v python3 >/dev/null 2>&1; then
    MOCK_PYHASH="${TMP}/mock_pyhash"
    mkdir -p "$MOCK_PYHASH"
    ln -sf "$(command -v python3)" "${MOCK_PYHASH}/python3"
    if PATH="$MOCK_PYHASH" _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/sums_dotslash" "asset.tar.gz"; then
        _pass "_debopty_verify_sha256 falls back to python3 when sha256sum/openssl absent"
    else
        _fail "_debopty_verify_sha256 python3 fallback failed"
    fi
fi

# Zero hashers in PATH -> fail closed
MOCK_NOHASH="${TMP}/mock_nohash"
mkdir -p "$MOCK_NOHASH"
if PATH="$MOCK_NOHASH" _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/sums_upper" "asset.tar.gz"; then
    _fail "_debopty_verify_sha256 must fail closed when no hash tool exists"
else
    _pass "_debopty_verify_sha256 fails closed with zero hashers"
fi

# 404 / missing asset triggers clean binary-install failure for graceful pure-Bash fallback
INST_404="${TMP}/inst_404"
mkdir -p "$INST_404"
if DEBOPTY_RELEASE_BASE_URL="file://${TMP}/nonexistent_404_release" \
   DEBOPTY_RELEASE_TAG="v1.0.7" \
   DEBOPTY_RS_BIN_LINK="${TMP}/debopty-rs-404" \
   _debopty_install_rust_binary "$INST_404" "false"; then
    _fail "_debopty_install_rust_binary should fail on missing 404 release"
else
    if [[ ! -e "${INST_404}/bin/debopty" && ! -e "${TMP}/debopty-rs-404" ]]; then
        _pass "_debopty_install_rust_binary 404 failure leaves clean state for Bash fallback"
    else
        _fail "_debopty_install_rust_binary 404 left stale artifacts behind"
    fi
fi

# --- 8. U14 Red/Blue Regression checks (BE-P1-15, BE-P1-18, BE-P2-01) ---
# (a) DEBOPTY_REPO respected in install_url & repo_url
_assert_eq "$(DEBOPTY_REPO="AcmeOrg/Custom-Fork" _debopty_resolve_repo_url)" "https://github.com/AcmeOrg/Custom-Fork.git" "BE-P1-18: _debopty_resolve_repo_url honors DEBOPTY_REPO"
_assert_eq "$(DEBOPTY_REPO="AcmeOrg/Custom-Fork" _debopty_resolve_install_url)" "https://raw.githubusercontent.com/AcmeOrg/Custom-Fork/main/install.sh" "BE-P1-18: _debopty_resolve_install_url honors DEBOPTY_REPO"

# (b) CRLF SHA256SUMS accepted (BE-P2-01)
printf '%s  asset.tar.gz\r\n' "$VALID_HEX" > "${U6_SHA_DIR}/sums_crlf"
if _debopty_verify_sha256 "${U6_SHA_DIR}/asset.tar.gz" "${U6_SHA_DIR}/sums_crlf" "asset.tar.gz"; then
    _pass "BE-P2-01: _debopty_verify_sha256 handles CRLF SHA256SUMS"
else
    _fail "BE-P2-01: _debopty_verify_sha256 failed on CRLF SHA256SUMS"
fi

# (c) python3 timeout and atomic binary staging present in install.sh (BE-P2-01, BE-P1-15)
_assert_file_contains "${ROOT}/install.sh" "socket.setdefaulttimeout(30)" "BE-P2-01: python3 fallback sets socket timeout"
_assert_file_contains "${ROOT}/install.sh" 'mv -f "$staged_bin" "${install_dir}/bin/debopty"' "BE-P1-15: binary install uses atomic staging mv to prevent ETXTBSY"

# (d) Symlinked /etc/resolv.conf handled safely without failing on dangling systemd-resolved symlink (BE-P1-18)
DNS_DIR="${TMP}/dns_test"
mkdir -p "$DNS_DIR"
ln -sf "/nonexistent/systemd/stub-resolv.conf" "${DNS_DIR}/resolv.conf"
if _debopty_write_temp_dns "${DNS_DIR}/resolv.conf"; then
    if [[ ! -L "${DNS_DIR}/resolv.conf" ]] && grep -qF "nameserver 1.1.1.1" "${DNS_DIR}/resolv.conf" && [[ "$(cat "${DNS_DIR}/resolv.conf.debopti.symlink.bak")" == "/nonexistent/systemd/stub-resolv.conf" ]]; then
        _pass "BE-P1-18: _debopty_write_temp_dns replaces dangling symlink safely and records backup"
    else
        _fail "BE-P1-18: _debopty_write_temp_dns did not safely replace symlink or record backup"
    fi
else
    _fail "BE-P1-18: _debopty_write_temp_dns failed on dangling symlink"
fi

# (e) Non-destructive staged git clone preserves existing install_dir on total mirror failure (BE-P1-18)
CLONE_INST="${TMP}/clone_inst"
mkdir -p "${CLONE_INST}/bin"
printf 'existing-working-install\n' > "${CLONE_INST}/bin/sentinel"
MOCK_GIT_FAIL="${TMP}/mock_git_fail"
mkdir -p "$MOCK_GIT_FAIL"
cat > "${MOCK_GIT_FAIL}/git" <<'EOF'
#!/bin/bash
exit 128
EOF
chmod +x "${MOCK_GIT_FAIL}/git"
if PATH="${MOCK_GIT_FAIL}:${PATH}" _debopty_clone_install_dir "$CLONE_INST" "https://mirror1.invalid/repo.git" "https://mirror2.invalid/repo.git" >/dev/null 2>&1; then
    _fail "BE-P1-18: _debopty_clone_install_dir should fail when all mirrors fail"
else
    if [[ "$(cat "${CLONE_INST}/bin/sentinel")" == "existing-working-install" ]]; then
        _pass "BE-P1-18: failed clone preserves existing install_dir untouched"
    else
        _fail "BE-P1-18: failed clone destroyed existing install_dir"
    fi
fi

# (f) Staged clone swaps atomically when fallback mirror succeeds
MOCK_GIT_OK="${TMP}/mock_git_ok"
mkdir -p "$MOCK_GIT_OK"
cat > "${MOCK_GIT_OK}/git" <<'EOF'
#!/bin/bash
url=$2
dest=$3
if [[ "$url" == *"good.invalid"* ]]; then
    mkdir -p "$dest"
    printf 'new-cloned-version\n' > "${dest}/MARKER"
    exit 0
fi
exit 128
EOF
chmod +x "${MOCK_GIT_OK}/git"
if PATH="${MOCK_GIT_OK}:${PATH}" _debopty_clone_install_dir "$CLONE_INST" "https://bad.invalid/repo.git" "https://good.invalid/repo.git" >/dev/null 2>&1; then
    _assert_eq "$(cat "${CLONE_INST}/MARKER")" "new-cloned-version" "BE-P1-18: staged clone swaps atomically on mirror success"
else
    _fail "BE-P1-18: staged clone failed on fallback mirror"
fi

# --- 9. U21 Install Reliability, Soft-200 Rejection, Fallback & Rollback checks ---
_link_sys_cmds() {
    local dest_dir=$1
    shift
    mkdir -p "$dest_dir"
    local cmd resolved
    for cmd in "$@"; do
        if resolved="$(command -v "$cmd" 2>/dev/null)"; then
            ln -sf "$resolved" "${dest_dir}/${cmd}"
        fi
    done
}

_make_release_fixture() {
    local rel_dir=$1
    local tag=$2
    local arch=$3
    local bin_label=$4
    local include_bin=${5:-true}
    local asset_name="debopty-${tag}-linux-${arch}.tar.gz"
    mkdir -p "${rel_dir}/pkg"
    rm -rf "${rel_dir}/pkg/"*
    if [[ "$include_bin" == "true" ]]; then
        cat > "${rel_dir}/pkg/debopty" <<EOF
#!/bin/bash
printf '%s:%s\n' "${bin_label}" "\$*"
EOF
        chmod 0755 "${rel_dir}/pkg/debopty"
        tar -czf "${rel_dir}/${asset_name}" -C "${rel_dir}/pkg" debopty
    else
        printf 'not-a-debopty-binary\n' > "${rel_dir}/pkg/placeholder.txt"
        tar -czf "${rel_dir}/${asset_name}" -C "${rel_dir}/pkg" placeholder.txt
    fi
    (cd "$rel_dir" && sha256sum "$asset_name" > SHA256SUMS)
}

# 9a: API 403 -> github.com/releases/latest HTML Fallback & ghfast.top Exclusion
REL_9A="${TMP}/u21_9a_rel"
_make_release_fixture "$REL_9A" "v1.2.3" "$HOST_ARCH" "u21-9a-bin"
MOCK_9A="${TMP}/mock_9a"
_link_sys_cmds "$MOCK_9A" tar gzip sha256sum mktemp mkdir cp chmod mv ln rm dirname readlink uname
CURL_LOG_9A="${TMP}/u21_9a_curl.log"
: > "$CURL_LOG_9A"
cat > "${MOCK_9A}/curl" <<EOF
#!/bin/bash
url=""
dest=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in
        -o) dest=\$2; shift 2 ;;
        -A|-H|--user-agent|--connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=\$1; shift ;;
    esac
done
printf '%s\n' "\$url" >> "$CURL_LOG_9A"
if [[ "\$url" == *"api.github.com"* ]]; then
    echo "curl: (22) The requested URL returned error: 403" >&2
    exit 22
elif [[ "\$url" == *"github.com/"*"/releases/latest"* ]]; then
    printf '<link rel="canonical" href="https://github.com/G3arB0xX/Debian-Optimizer-Script/releases/tag/v1.2.3">\n' > "\$dest"
    exit 0
elif [[ "\$url" == *"/releases/download/v1.2.3/SHA256SUMS" ]]; then
    cp "${REL_9A}/SHA256SUMS" "\$dest"
    exit 0
elif [[ "\$url" == *"/debopty-v1.2.3-linux-${HOST_ARCH}.tar.gz" ]]; then
    cp "${REL_9A}/debopty-v1.2.3-linux-${HOST_ARCH}.tar.gz" "\$dest"
    exit 0
fi
exit 22
EOF
chmod +x "${MOCK_9A}/curl"

INST_9A="${TMP}/inst_9a"
mkdir -p "$INST_9A"
if PATH="$MOCK_9A" \
   DEBOPTY_RELEASE_TAG="" \
   DEBOPTY_RELEASE_BASE_URL="" \
   DEBOPTY_BIN_LINK="${TMP}/bin_9a" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/f2b_9a" \
   DEBOPTY_RS_BIN_LINK="${TMP}/rs_9a" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/legacy_bin_9a" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/legacy_opt_9a" \
   DEBOPTY_PROFILE_DIR="${TMP}/profile_9a" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/fish_9a" \
   _debopty_install_rust_binary "$INST_9A" "true"; then
    if [[ -x "${INST_9A}/bin/debopty" ]]; then
        _assert_eq "$("${INST_9A}/bin/debopty" ok)" "u21-9a-bin:ok" "9a: API 403 falls back to releases/latest HTML and installs binary"
    else
        _fail "9a: binary not executable after API 403 fallback"
    fi
else
    _fail "9a: _debopty_install_rust_binary failed on API 403 fallback"
fi

first_api_line="$(grep -n 'api\.github\.com' "$CURL_LOG_9A" | head -n1 | cut -d: -f1 || true)"
first_rel_line="$(grep -n '/releases/latest' "$CURL_LOG_9A" | grep -v 'api\.github\.com' | head -n1 | cut -d: -f1 || true)"
if [[ -n "$first_api_line" && -n "$first_rel_line" && "$first_api_line" -lt "$first_rel_line" ]]; then
    _pass "9a: api.github.com tried first, then github.com/releases/latest"
else
    _fail "9a: unexpected lookup order (api_line=${first_api_line:-none}, rel_line=${first_rel_line:-none})"
fi
if grep -E 'api\.github\.com|/releases/latest' "$CURL_LOG_9A" | grep -qF 'ghfast.top'; then
    _fail "9a: ghfast.top must never be used for API or /releases/latest"
else
    _pass "9a: ghfast.top excluded from API and /releases/latest URLs"
fi

# 9b: Compact Single-Line JSON Without Trailing Newline & Unterminated SHA256SUMS / VERSION
printf '{"tag_name":"v1.2.4"}' > "${TMP}/u21_9b_api.json"
_assert_eq "$(_debopty_extract_api_tag "${TMP}/u21_9b_api.json" || true)" "v1.2.4" "9b: _debopty_extract_api_tag parses single-line JSON without trailing newline"

printf 'u21-9b-payload\n' > "${TMP}/u21_9b_asset.tar.gz"
DIGEST_9B="$(sha256sum "${TMP}/u21_9b_asset.tar.gz" | awk '{print $1}')"
printf '%s  %s' "$DIGEST_9B" "u21_9b_asset.tar.gz" > "${TMP}/u21_9b_sums"
if _debopty_verify_sha256 "${TMP}/u21_9b_asset.tar.gz" "${TMP}/u21_9b_sums" "u21_9b_asset.tar.gz"; then
    _pass "9b: _debopty_verify_sha256 passes on SHA256SUMS without trailing newline"
else
    _fail "9b: _debopty_verify_sha256 failed on SHA256SUMS without trailing newline"
fi

REL_9B="${TMP}/u21_9b_rel"
_make_release_fixture "$REL_9B" "v1.2.5" "$HOST_ARCH" "u21-9b-bin"
INST_9B="${TMP}/inst_9b"
mkdir -p "$INST_9B"
printf 'DEBOPTI_SEMVER="v1.2.5"' > "${INST_9B}/VERSION"
if DEBOPTY_RELEASE_BASE_URL="file://${REL_9B}" \
   DEBOPTY_RELEASE_TAG="" \
   DEBOPTY_BIN_LINK="${TMP}/bin_9b" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/f2b_9b" \
   DEBOPTY_RS_BIN_LINK="${TMP}/rs_9b" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/legacy_bin_9b" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/legacy_opt_9b" \
   DEBOPTY_PROFILE_DIR="${TMP}/profile_9b" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/fish_9b" \
   _debopty_install_rust_binary "$INST_9B" "false"; then
    _assert_eq "$("${INST_9B}/bin/debopty" ver)" "u21-9b-bin:ver" "9b: unterminated VERSION with leading v resolves v1.2.5 (no vv duplication)"
else
    _fail "9b: _debopty_install_rust_binary failed on unterminated VERSION with leading v"
fi

# 9c: CalVer v2026.08.16.1 Rejection (No Truncation to v2026.08.16)
printf '{"tag_name":"v2026.08.16.1"}\n' > "${TMP}/u21_9c_calver.json"
if _debopty_extract_api_tag "${TMP}/u21_9c_calver.json" >/dev/null 2>&1; then
    _fail "9c: _debopty_extract_api_tag must reject CalVer v2026.08.16.1"
else
    _pass "9c: _debopty_extract_api_tag rejects CalVer v2026.08.16.1"
fi

printf '<a href="/org/repo/releases/tag/v2026.08.16.1">old</a>\n' > "${TMP}/u21_9c_calver.html"
calver_page_out=""
if calver_page_out="$(_debopty_extract_release_page_tag "${TMP}/u21_9c_calver.html" 2>/dev/null)"; then
    _fail "9c: _debopty_extract_release_page_tag must reject CalVer v2026.08.16.1 (got='$calver_page_out')"
else
    _assert_eq "$calver_page_out" "" "9c: _debopty_extract_release_page_tag rejects CalVer without truncating to v2026.08.16"
fi

printf '<a href="/org/repo/releases/tag/v2026.08.16.1">old</a> <a href="/org/repo/releases/tag/v1.2.6">new</a>\n' > "${TMP}/u21_9c_mixed.html"
_assert_eq "$(_debopty_extract_release_page_tag "${TMP}/u21_9c_mixed.html" || true)" "v1.2.6" "9c: _debopty_extract_release_page_tag skips CalVer and extracts v1.2.6 on same line"

# 9d: CN Soft-200 HTML Rejection Across Mirror Loop + Zero apt-get Thrashing
REL_9D="${TMP}/u21_9d_rel"
_make_release_fixture "$REL_9D" "v1.2.7" "$HOST_ARCH" "u21-9d-bin"
MOCK_9D="${TMP}/mock_9d"
_link_sys_cmds "$MOCK_9D" tar gzip sha256sum mktemp mkdir cp chmod mv ln rm dirname readlink uname
CURL_LOG_9D="${TMP}/u21_9d_curl.log"
APT_SPY_LOG="${TMP}/u21_9d_apt_spy.log"
: > "$CURL_LOG_9D"
: > "$APT_SPY_LOG"
cat > "${MOCK_9D}/apt-get" <<EOF
#!/bin/bash
printf '%s\n' "\$*" >> "$APT_SPY_LOG"
exit 1
EOF
chmod +x "${MOCK_9D}/apt-get"

cat > "${MOCK_9D}/curl" <<EOF
#!/bin/bash
url=""
dest=""
while [[ \$# -gt 0 ]]; do
    case "\$1" in
        -o) dest=\$2; shift 2 ;;
        -A|-H|--user-agent|--connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=\$1; shift ;;
    esac
done
printf '%s\n' "\$url" >> "$CURL_LOG_9D"
if [[ "\$url" == *"gh-proxy.com/"* ]]; then
    printf '<!DOCTYPE html><html><head><title>Just a moment...</title></head><body>Cloudflare</body></html>\n' > "\$dest"
    exit 0
elif [[ "\$url" == *"edgeone.gh-proxy.org/https://api.github.com"* ]]; then
    printf '{"tag_name":"v1.2.7"}' > "\$dest"
    exit 0
elif [[ "\$url" == *"cdn.gh-proxy.org/"*"/SHA256SUMS" ]]; then
    cp "${REL_9D}/SHA256SUMS" "\$dest"
    exit 0
elif [[ "\$url" == *"cdn.gh-proxy.org/"*"/debopty-v1.2.7-linux-${HOST_ARCH}.tar.gz" ]]; then
    cp "${REL_9D}/debopty-v1.2.7-linux-${HOST_ARCH}.tar.gz" "\$dest"
    exit 0
fi
exit 22
EOF
chmod +x "${MOCK_9D}/curl"

INST_9D="${TMP}/inst_9d"
mkdir -p "$INST_9D"
if PATH="$MOCK_9D" \
   DEBOPTY_RELEASE_TAG="" \
   DEBOPTY_RELEASE_BASE_URL="" \
   DEBOPTY_BIN_LINK="${TMP}/bin_9d" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/f2b_9d" \
   DEBOPTY_RS_BIN_LINK="${TMP}/rs_9d" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/legacy_bin_9d" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/legacy_opt_9d" \
   DEBOPTY_PROFILE_DIR="${TMP}/profile_9d" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/fish_9d" \
   _debopty_install_rust_binary "$INST_9D" "true"; then
    if [[ -x "${INST_9D}/bin/debopty" ]]; then
        _assert_eq "$("${INST_9D}/bin/debopty" ok)" "u21-9d-bin:ok" "9d: rejects soft-200 HTML on gh-proxy.com and succeeds via fallback mirrors"
    else
        _fail "9d: binary not executable after mirror fallback"
    fi
else
    _fail "9d: _debopty_install_rust_binary failed when gh-proxy.com returned soft-200 HTML"
fi
_assert_file_contains "$CURL_LOG_9D" "gh-proxy.com" "9d: gh-proxy.com was attempted and rejected"
_assert_file_contains "$CURL_LOG_9D" "edgeone.gh-proxy.org" "9d: edgeone.gh-proxy.org API fallback succeeded"
_assert_file_contains "$CURL_LOG_9D" "cdn.gh-proxy.org" "9d: cdn.gh-proxy.org asset fallback succeeded"
if [[ ! -s "$APT_SPY_LOG" ]]; then
    _pass "9d: zero apt-get invocations during mirror loop failures"
else
    _fail "9d: apt-get was unexpectedly invoked ($(cat "$APT_SPY_LOG"))"
fi

# Direct _debopty_bootstrap_fetch HTTP 404 (curl rc=22) must NOT invoke apt-get
: > "$APT_SPY_LOG"
if PATH="$MOCK_9D" _debopty_bootstrap_fetch "https://example.invalid/404" "${TMP}/u21_9d_404.out"; then
    _fail "9d: _debopty_bootstrap_fetch should fail on curl exit 22"
else
    if [[ ! -s "$APT_SPY_LOG" ]]; then
        _pass "9d: _debopty_bootstrap_fetch curl rc=22 fast-fails with zero apt-get calls"
    else
        _fail "9d: _debopty_bootstrap_fetch invoked apt-get on curl rc=22"
    fi
fi

# 9e: Per-Stage Structured Stderr Diagnostics (stage=...)
_run_diag_install() {
    local label=$1
    local path_val=$2
    local base_url=$3
    local tag=$4
    local out_file="${TMP}/diag_${label}.stdout"
    local err_file="${TMP}/diag_${label}.stderr"
    if PATH="$path_val" \
       DEBOPTY_RELEASE_BASE_URL="$base_url" \
       DEBOPTY_RELEASE_TAG="$tag" \
       DEBOPTY_BIN_LINK="${TMP}/bin_9e" \
       DEBOPTY_F2B_BIN_LINK="${TMP}/f2b_9e" \
       DEBOPTY_RS_BIN_LINK="${TMP}/rs_9e" \
       DEBOPTY_LEGACY_BIN_LINK="${TMP}/legacy_bin_9e" \
       DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/legacy_opt_9e" \
       DEBOPTY_PROFILE_DIR="${TMP}/profile_9e" \
       DEBOPTY_FISH_CONF_DIR="${TMP}/fish_9e" \
       _debopty_install_rust_binary "$INST_9E" "false" >"$out_file" 2>"$err_file"; then
        _fail "9e (${label}): expected non-zero failure"
        return 0
    fi
    if [[ ! -s "$out_file" ]]; then
        _pass "9e (${label}): stdout remains empty on failure"
    else
        _fail "9e (${label}): unexpected stdout output"
    fi
}

INST_9E="${TMP}/inst_9e"
mkdir -p "$INST_9E"

# 9e.1 unsupported arch
_run_diag_install "arch" "${MOCK_UNAME}:${PATH}" "file://${REL_9D}" "v1.2.8"
_assert_file_contains "${TMP}/diag_arch.stderr" "[stage=arch]" "9e.1: stderr includes [stage=arch]"
_assert_file_contains "${TMP}/diag_arch.stderr" "riscv64" "9e.1: stderr includes unsupported arch riscv64"

# 9e.2 missing SHA256SUMS
_run_diag_install "sha256sums" "$PATH" "file://${TMP}/nonexistent_9e" "v1.2.8"
_assert_file_contains "${TMP}/diag_sha256sums.stderr" "[stage=sha256sums]" "9e.2: stderr includes [stage=sha256sums]"
_assert_file_contains "${TMP}/diag_sha256sums.stderr" "v1.2.8" "9e.2: stderr includes tag v1.2.8"
_assert_file_contains "${TMP}/diag_sha256sums.stderr" "$HOST_ARCH" "9e.2: stderr includes arch ${HOST_ARCH}"

# 9e.3 SHA256SUMS missing target asset entry
REL_9E_ENTRY="${TMP}/u21_9e_missing_entry"
mkdir -p "$REL_9E_ENTRY"
ASSET_9E="debopty-v1.2.8-linux-${HOST_ARCH}.tar.gz"
printf '0000000000000000000000000000000000000000000000000000000000000000  other-asset.tar.gz\n' > "${REL_9E_ENTRY}/SHA256SUMS"
_run_diag_install "entry" "$PATH" "file://${REL_9E_ENTRY}" "v1.2.8"
_assert_file_contains "${TMP}/diag_entry.stderr" "[stage=sha256sums-entry]" "9e.3: stderr includes [stage=sha256sums-entry]"
_assert_file_contains "${TMP}/diag_entry.stderr" "$ASSET_9E" "9e.3: stderr includes missing asset ${ASSET_9E}"

# 9e.4 SHA-256 mismatch
REL_9E_BADSHA="${TMP}/u21_9e_badsha"
_make_release_fixture "$REL_9E_BADSHA" "v1.2.8" "$HOST_ARCH" "u21-9e-bin"
printf '0000000000000000000000000000000000000000000000000000000000000000  %s\n' "$ASSET_9E" > "${REL_9E_BADSHA}/SHA256SUMS"
_run_diag_install "sha256" "$PATH" "file://${REL_9E_BADSHA}" "v1.2.8"
_assert_file_contains "${TMP}/diag_sha256.stderr" "[stage=sha256]" "9e.4: stderr includes [stage=sha256]"
_assert_file_contains "${TMP}/diag_sha256.stderr" "expected=" "9e.4: stderr includes expected="
_assert_file_contains "${TMP}/diag_sha256.stderr" "actual=" "9e.4: stderr includes actual="

# 9e.5 valid .tar.gz without debopty binary inside
REL_9E_NOBIN="${TMP}/u21_9e_nobin"
_make_release_fixture "$REL_9E_NOBIN" "v1.2.8" "$HOST_ARCH" "unused" "false"
_run_diag_install "binary" "$PATH" "file://${REL_9E_NOBIN}" "v1.2.8"
_assert_file_contains "${TMP}/diag_binary.stderr" "[stage=binary]" "9e.5: stderr includes [stage=binary]"

# 9f: Non-Destructive Legacy-Bash Upgrade, Preservation on Failure & Atomic Cutover on Success
U21_LEGACY_OPT="${TMP}/u21_legacy_opt"
mkdir -p "$U21_LEGACY_OPT"
printf '#!/bin/bash\necho legacy-ok\n' > "${U21_LEGACY_OPT}/deb_optimizer.sh"
chmod 0755 "${U21_LEGACY_OPT}/deb_optimizer.sh"
U21_BIN_LINK="${TMP}/u21_bin_link"
U21_F2B_LINK="${TMP}/u21_f2b_link"
U21_RS_LINK="${TMP}/u21_rs_link"
ln -sf "${U21_LEGACY_OPT}/deb_optimizer.sh" "$U21_BIN_LINK"

REPO_FIXTURE_9F="${TMP}/u21_repo_src"
mkdir -p "${REPO_FIXTURE_9F}/rust" "${REPO_FIXTURE_9F}/archive/legacy-bash" "${REPO_FIXTURE_9F}/.agents" "${REPO_FIXTURE_9F}/governance" "${REPO_FIXTURE_9F}/docs" "${REPO_FIXTURE_9F}/templates/common"
printf 'DEBOPTI_SEMVER="1.2.9"\n' > "${REPO_FIXTURE_9F}/VERSION"
printf '[workspace]\nmembers = []\n' > "${REPO_FIXTURE_9F}/rust/Cargo.toml"
printf '#!/bin/bash\necho archived-legacy\n' > "${REPO_FIXTURE_9F}/archive/legacy-bash/deb_optimizer.sh"
printf '# constraints\n' > "${REPO_FIXTURE_9F}/.agents/CONSTRAINTS.md"
printf '# constraints\n' > "${REPO_FIXTURE_9F}/governance/CONSTRAINTS.md"
printf '# root index\n' > "${REPO_FIXTURE_9F}/INDEX.md"
printf '# docs index\n' > "${REPO_FIXTURE_9F}/docs/INDEX.md"
printf 'alias debopty="BIN_LINK"\n' > "${REPO_FIXTURE_9F}/templates/common/debopty.sh"

MOCK_9F="${TMP}/mock_9f"
_link_sys_cmds "$MOCK_9F" tar gzip sha256sum mktemp mkdir cp chmod mv ln rm dirname readlink uname find
cat > "${MOCK_9F}/git" <<EOF
#!/bin/bash
if [[ "\${1:-}" == "clone" ]]; then
    dest="\${3:-}"
    mkdir -p "\$dest"
    cp -R "${REPO_FIXTURE_9F}/." "\$dest/"
    exit 0
fi
exit 1
EOF
chmod +x "${MOCK_9F}/git"

cat > "${MOCK_9F}/curl" <<'EOF'
#!/bin/bash
url=""
dest=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        -o) dest=$2; shift 2 ;;
        -A|-H|--user-agent|--connect-timeout|--max-time) shift 2 ;;
        -*) shift ;;
        *) url=$1; shift ;;
    esac
done
if [[ "$url" == file://* ]]; then
    path="${url#file://}"
    if [[ -f "$path" ]]; then
        cp "$path" "$dest"
        exit 0
    fi
fi
exit 22
EOF
chmod +x "${MOCK_9F}/curl"

# 9f failure case (binary 404 preserves legacy install & symlink cleanly)
ERR_9F_FAIL="${TMP}/u21_9f_fail.stderr"
if PATH="$MOCK_9F" \
   DEBOPTY_RELEASE_BASE_URL="file://${TMP}/nonexistent_9f_rel" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_SOURCE_FALLBACK=0 \
   DEBOPTY_BIN_LINK="$U21_BIN_LINK" \
   DEBOPTY_F2B_BIN_LINK="$U21_F2B_LINK" \
   DEBOPTY_RS_BIN_LINK="$U21_RS_LINK" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/u21_unused_legacy_link" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/u21_unused_legacy_opt" \
   DEBOPTY_PROFILE_DIR="${TMP}/u21_profile" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/u21_fish" \
   _debopty_upgrade_install_dir "$U21_LEGACY_OPT" "false" "https://mock.invalid/repo.git" 2>"$ERR_9F_FAIL"; then
    _fail "9f: _debopty_upgrade_install_dir should fail when binary download 404s"
else
    _pass "9f: _debopty_upgrade_install_dir returns non-zero on binary 404"
fi
_assert_file_contains "${U21_LEGACY_OPT}/deb_optimizer.sh" "echo legacy-ok" "9f: failure preserves existing deb_optimizer.sh intact"
_assert_eq "$(readlink "$U21_BIN_LINK" || true)" "${U21_LEGACY_OPT}/deb_optimizer.sh" "9f: failure preserves symlink to deb_optimizer.sh (not dangling)"
_assert_file_contains "$ERR_9F_FAIL" "已保留原有" "9f: failure stderr notes existing install preserved"
if compgen -G "${U21_LEGACY_OPT}.staging.*" >/dev/null || compgen -G "${U21_LEGACY_OPT}.old.*" >/dev/null; then
    _fail "9f: failure left behind .staging or .old directory"
else
    _pass "9f: failure leaves zero .staging/.old leftover directories"
fi

# 9f success case (valid file:// binary swaps atomically and strips dev metadata)
REL_9F="${TMP}/u21_9f_rel"
_make_release_fixture "$REL_9F" "v1.2.9" "$HOST_ARCH" "u21-9f-bin"
if PATH="$MOCK_9F" \
   DEBOPTY_RELEASE_BASE_URL="file://${REL_9F}" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_BIN_LINK="$U21_BIN_LINK" \
   DEBOPTY_F2B_BIN_LINK="$U21_F2B_LINK" \
   DEBOPTY_RS_BIN_LINK="$U21_RS_LINK" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/u21_unused_legacy_link" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/u21_unused_legacy_opt" \
   DEBOPTY_PROFILE_DIR="${TMP}/u21_profile" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/u21_fish" \
   _debopty_upgrade_install_dir "$U21_LEGACY_OPT" "false" "https://mock.invalid/repo.git" 2>/dev/null; then
    if [[ -x "${U21_LEGACY_OPT}/bin/debopty" ]]; then
        _assert_eq "$("${U21_LEGACY_OPT}/bin/debopty" cutover)" "u21-9f-bin:cutover" "9f: success installs executable bin/debopty"
    else
        _fail "9f: success did not produce executable bin/debopty"
    fi
    _assert_eq "$(readlink "$U21_BIN_LINK" || true)" "${U21_LEGACY_OPT}/bin/debopty" "9f: bin_link updated to bin/debopty"
    _assert_eq "$(readlink "$U21_F2B_LINK" || true)" "${U21_LEGACY_OPT}/bin/debopty" "9f: f2b_link updated to bin/debopty"
    if [[ ! -e "${U21_LEGACY_OPT}/rust" && ! -e "${U21_LEGACY_OPT}/archive" && ! -e "${U21_LEGACY_OPT}/.agents" && ! -e "${U21_LEGACY_OPT}/governance" && ! -e "${U21_LEGACY_OPT}/INDEX.md" && ! -e "${U21_LEGACY_OPT}/docs/INDEX.md" ]]; then
        _pass "9f: rust/, archive/, .agents/, governance/, and INDEX.md all stripped after binary staging"
    else
        _fail "9f: dev/legacy metadata remained after upgrade"
    fi
else
    _fail "9f: _debopty_upgrade_install_dir failed on valid file:// release"
fi

# 9f opt-in DEBOPTY_SOURCE_FALLBACK=1 case
cat > "${MOCK_9F}/cargo" <<'EOF'
#!/bin/bash
if [[ ! -f "${PWD}/Cargo.toml" ]]; then
    echo "missing Cargo.toml in ${PWD}" >&2
    exit 1
fi
mkdir -p "${PWD}/target/release"
printf '%s\n' '#!/bin/bash' 'printf "u21-9f-cargo-bin:%s\n" "$*"' > "${PWD}/target/release/debopty"
chmod 0755 "${PWD}/target/release/debopty"
EOF
chmod +x "${MOCK_9F}/cargo"

U21_SRC_OPT="${TMP}/u21_src_opt"
if PATH="$MOCK_9F" \
   DEBOPTY_RELEASE_BASE_URL="file://${TMP}/nonexistent_9f_rel" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_SOURCE_FALLBACK=1 \
   DEBOPTY_BIN_LINK="${TMP}/u21_src_bin_link" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/u21_src_f2b_link" \
   DEBOPTY_RS_BIN_LINK="${TMP}/u21_src_rs_link" \
   DEBOPTY_LEGACY_BIN_LINK="${TMP}/u21_unused_legacy_link" \
   DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/u21_unused_legacy_opt" \
   DEBOPTY_PROFILE_DIR="${TMP}/u21_src_profile" \
   DEBOPTY_FISH_CONF_DIR="${TMP}/u21_src_fish" \
   _debopty_upgrade_install_dir "$U21_SRC_OPT" "false" "https://mock.invalid/repo.git" 2>/dev/null; then
    if [[ -x "${U21_SRC_OPT}/bin/debopty" && ! -e "${U21_SRC_OPT}/rust" ]]; then
        _assert_eq "$("${U21_SRC_OPT}/bin/debopty" built)" "u21-9f-cargo-bin:built" "9f: DEBOPTY_SOURCE_FALLBACK=1 builds binary before stripping rust/"
    else
        _fail "9f: DEBOPTY_SOURCE_FALLBACK=1 missing binary or failed to strip rust/ afterward"
    fi
else
    _fail "9f: DEBOPTY_SOURCE_FALLBACK=1 upgrade failed"
fi

# 9g: U22 Pre-Rename debopti Migration & Safety
# (a) Cleans symlink debopti, wrapper-script debopti, debopti.sh, debopti.fish, and marker-bearing legacy dir
U22_INST="${TMP}/u22_inst"
mkdir -p "$U22_INST"
U22_PROF="${TMP}/u22_profile"
U22_FISH="${TMP}/u22_fish"
mkdir -p "$U22_PROF" "$U22_FISH"
printf 'alias debopti=old\n' > "${U22_PROF}/debopti.sh"
printf 'abbr -a debopti /usr/local/bin/debopti\n' > "${U22_FISH}/debopti.fish"

U22_LEGACY_DIR_MARKED="${TMP}/u22_opt_debopti_marked"
mkdir -p "$U22_LEGACY_DIR_MARKED"
printf '#!/bin/bash\necho pre-rename\n' > "${U22_LEGACY_DIR_MARKED}/deb_optimizer.sh"

U22_LEGACY_LINK_SYM="${TMP}/u22_debopti_symlink"
ln -sf "/opt/debopti/deb_optimizer.sh" "$U22_LEGACY_LINK_SYM"
if DEBOPTY_RELEASE_BASE_URL="file://${REL_9F}" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_BIN_LINK="${TMP}/u22_bin_link_sym" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/u22_f2b_link_sym" \
   DEBOPTY_RS_BIN_LINK="${TMP}/u22_rs_link_sym" \
   DEBOPTY_LEGACY_BIN_LINK="$U22_LEGACY_LINK_SYM" \
   DEBOPTY_LEGACY_INSTALL_DIR="$U22_LEGACY_DIR_MARKED" \
   DEBOPTY_PROFILE_DIR="$U22_PROF" \
   DEBOPTY_FISH_CONF_DIR="$U22_FISH" \
   _debopty_install_rust_binary "$U22_INST" "false"; then
    if [[ ! -L "$U22_LEGACY_LINK_SYM" && ! -e "$U22_LEGACY_LINK_SYM" && ! -e "${U22_PROF}/debopti.sh" && ! -e "${U22_FISH}/debopti.fish" && ! -e "$U22_LEGACY_DIR_MARKED" ]]; then
        _pass "9g: cleans legacy symlink debopti, debopti.sh, debopti.fish, and marker-bearing legacy install dir"
    else
        _fail "9g: failed to clean legacy symlink debopti or hooks/dir"
    fi
else
    _fail "9g: _debopty_install_rust_binary failed during symlink migration check"
fi

mkdir -p "$U22_LEGACY_DIR_MARKED"
printf '#!/bin/bash\necho pre-rename\n' > "${U22_LEGACY_DIR_MARKED}/deb_optimizer.sh"
printf 'alias debopti=old\n' > "${U22_PROF}/debopti.sh"
printf 'abbr -a debopti /usr/local/bin/debopti\n' > "${U22_FISH}/debopti.fish"
U22_LEGACY_LINK_WRAPPER="${TMP}/u22_debopti_wrapper"
printf '#!/bin/bash\nexec /opt/debopti/deb_optimizer.sh "$@"\n' > "$U22_LEGACY_LINK_WRAPPER"
chmod 0755 "$U22_LEGACY_LINK_WRAPPER"

if DEBOPTY_RELEASE_BASE_URL="file://${REL_9F}" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_BIN_LINK="${TMP}/u22_bin_link" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/u22_f2b_link" \
   DEBOPTY_RS_BIN_LINK="${TMP}/u22_rs_link" \
   DEBOPTY_LEGACY_BIN_LINK="$U22_LEGACY_LINK_WRAPPER" \
   DEBOPTY_LEGACY_INSTALL_DIR="$U22_LEGACY_DIR_MARKED" \
   DEBOPTY_PROFILE_DIR="$U22_PROF" \
   DEBOPTY_FISH_CONF_DIR="$U22_FISH" \
   _debopty_install_rust_binary "$U22_INST" "false"; then
    if [[ ! -e "$U22_LEGACY_LINK_WRAPPER" && ! -e "${U22_PROF}/debopti.sh" && ! -e "${U22_FISH}/debopti.fish" && ! -e "$U22_LEGACY_DIR_MARKED" ]]; then
        _pass "9g: cleans wrapper debopti, debopti.sh, debopti.fish, and marker-bearing legacy install dir"
    else
        _fail "9g: failed to clean one or more legacy debopti artifacts"
    fi
else
    _fail "9g: _debopty_install_rust_binary failed during pre-rename migration check"
fi

# (b) Preserves unrelated user script at DEBOPTY_LEGACY_BIN_LINK & markerless legacy install dir
U22_USER_SCRIPT="${TMP}/u22_debopti_user_script"
printf '#!/bin/sh\necho user-tool\n' > "$U22_USER_SCRIPT"
chmod 0755 "$U22_USER_SCRIPT"
U22_UNMARKED_DIR="${TMP}/u22_opt_debopti_unmarked"
mkdir -p "$U22_UNMARKED_DIR"
printf 'custom user data\n' > "${U22_UNMARKED_DIR}/user_only.txt"

if DEBOPTY_RELEASE_BASE_URL="file://${REL_9F}" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_BIN_LINK="${TMP}/u22_bin_link2" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/u22_f2b_link2" \
   DEBOPTY_RS_BIN_LINK="${TMP}/u22_rs_link2" \
   DEBOPTY_LEGACY_BIN_LINK="$U22_USER_SCRIPT" \
   DEBOPTY_LEGACY_INSTALL_DIR="$U22_UNMARKED_DIR" \
   DEBOPTY_PROFILE_DIR="$U22_PROF" \
   DEBOPTY_FISH_CONF_DIR="$U22_FISH" \
   _debopty_install_rust_binary "$U22_INST" "false"; then
    if [[ -f "$U22_USER_SCRIPT" && -f "${U22_UNMARKED_DIR}/user_only.txt" ]]; then
        _pass "9g: preserves unrelated user script and markerless directory"
    else
        _fail "9g: deleted unrelated user script or markerless directory"
    fi
else
    _fail "9g: _debopty_install_rust_binary failed during user-script preservation check"
fi

# (c) Failed _debopty_upgrade_install_dir on pre-rename-only VPS leaves debopti untouched
U22_FAIL_NEW_OPT="${TMP}/u22_fail_new_debopty"
U22_FAIL_OLD_OPT="${TMP}/u22_fail_old_debopti"
U22_FAIL_OLD_LINK="${TMP}/u22_fail_old_debopti_link"
U22_FAIL_PROF="${TMP}/u22_fail_profile"
U22_FAIL_FISH="${TMP}/u22_fail_fish"
mkdir -p "$U22_FAIL_OLD_OPT" "$U22_FAIL_PROF" "$U22_FAIL_FISH"
printf '#!/bin/bash\necho keep-pre-rename\n' > "${U22_FAIL_OLD_OPT}/deb_optimizer.sh"
ln -sf "${U22_FAIL_OLD_OPT}/deb_optimizer.sh" "$U22_FAIL_OLD_LINK"
printf 'alias debopti=keep\n' > "${U22_FAIL_PROF}/debopti.sh"
printf 'abbr -a debopti /usr/local/bin/debopti\n' > "${U22_FAIL_FISH}/debopti.fish"

if PATH="$MOCK_9F" \
   DEBOPTY_RELEASE_BASE_URL="file://${TMP}/nonexistent_u22" \
   DEBOPTY_RELEASE_TAG="v1.2.9" \
   DEBOPTY_SOURCE_FALLBACK=0 \
   DEBOPTY_BIN_LINK="${TMP}/u22_fail_new_link" \
   DEBOPTY_F2B_BIN_LINK="${TMP}/u22_fail_new_f2b" \
   DEBOPTY_RS_BIN_LINK="${TMP}/u22_fail_new_rs" \
   DEBOPTY_LEGACY_BIN_LINK="$U22_FAIL_OLD_LINK" \
   DEBOPTY_LEGACY_INSTALL_DIR="$U22_FAIL_OLD_OPT" \
   DEBOPTY_PROFILE_DIR="$U22_FAIL_PROF" \
   DEBOPTY_FISH_CONF_DIR="$U22_FAIL_FISH" \
   _debopty_upgrade_install_dir "$U22_FAIL_NEW_OPT" "false" "https://mock.invalid/repo.git" 2>/dev/null; then
    _fail "9g: pre-rename-only VPS upgrade should fail on missing release"
else
    if [[ ! -e "$U22_FAIL_NEW_OPT" ]] && \
       grep -qF 'echo keep-pre-rename' "${U22_FAIL_OLD_OPT}/deb_optimizer.sh" && \
       [[ "$(readlink "$U22_FAIL_OLD_LINK" || true)" == "${U22_FAIL_OLD_OPT}/deb_optimizer.sh" ]] && \
       [[ -f "${U22_FAIL_PROF}/debopti.sh" && -f "${U22_FAIL_FISH}/debopti.fish" ]]; then
        _pass "9g: failed upgrade on pre-rename-only VPS leaves debopti install and hooks 100% untouched"
    else
        _fail "9g: failed upgrade modified pre-rename debopti install or hooks"
    fi
fi

# 9h: U22 README Full-Name Headings & abbrs.fish Check
_assert_eq "$(head -n 1 "${ROOT}/README.md")" "# Debian Optimizer Script" "9h: README.md line 1 uses full name without debopty"
_assert_eq "$(head -n 1 "${ROOT}/README_CN.md")" "# Debian Optimizer Script" "9h: README_CN.md line 1 uses full name without debopty"
_assert_eq "$(head -n 1 "${ROOT}/docs/README.md")" "# Debian Optimizer Script — 手动操作文档" "9h: docs/README.md line 1 uses full name without debopty"
_assert_file_contains "${ROOT}/templates/apps/devops/abbrs.fish" "abbr -a debopty '/usr/local/bin/debopty'" "9h: abbrs.fish uses debopty"
_assert_file_not_contains "${ROOT}/templates/apps/devops/abbrs.fish" "abbr -a debopti" "9h: abbrs.fish removed stale abbr -a debopti"

# 9i: U28 _debopty_register_rust_binary auto-provisions agent workspace & honors DEBOPTY_SKIP_AGENT_WORKSPACE=1
U28_INST="${TMP}/u28_inst"
mkdir -p "${U28_INST}/bin"
U28_CALL_LOG="${TMP}/u28_agent_init.log"
: > "$U28_CALL_LOG"
cat > "${U28_INST}/bin/debopty" <<EOF
#!/bin/bash
printf '%s\n' "\$*" >> "$U28_CALL_LOG"
EOF
chmod 0755 "${U28_INST}/bin/debopty"
U28_WS="${TMP}/u28_agent_ws"

DEBOPTY_BIN_LINK="${TMP}/u28_bin_link" \
DEBOPTY_F2B_BIN_LINK="${TMP}/u28_f2b_link" \
DEBOPTY_RS_BIN_LINK="${TMP}/u28_rs_link" \
DEBOPTY_LEGACY_BIN_LINK="${TMP}/u28_unused_legacy_link" \
DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/u28_unused_legacy_opt" \
DEBOPTY_PROFILE_DIR="${TMP}/u28_profile" \
DEBOPTY_FISH_CONF_DIR="${TMP}/u28_fish" \
DEBOPTY_AGENT_WORKSPACE="$U28_WS" \
DEBOPTY_SKIP_AGENT_WORKSPACE=0 \
_debopty_register_rust_binary "$U28_INST"
_assert_file_contains "$U28_CALL_LOG" "agent init --dir ${U28_WS} --non-interactive" "9i: _debopty_register_rust_binary invokes agent init --dir <tmp> --non-interactive"

: > "$U28_CALL_LOG"
DEBOPTY_BIN_LINK="${TMP}/u28_bin_link2" \
DEBOPTY_F2B_BIN_LINK="${TMP}/u28_f2b_link2" \
DEBOPTY_RS_BIN_LINK="${TMP}/u28_rs_link2" \
DEBOPTY_LEGACY_BIN_LINK="${TMP}/u28_unused_legacy_link" \
DEBOPTY_LEGACY_INSTALL_DIR="${TMP}/u28_unused_legacy_opt" \
DEBOPTY_PROFILE_DIR="${TMP}/u28_profile" \
DEBOPTY_FISH_CONF_DIR="${TMP}/u28_fish" \
DEBOPTY_AGENT_WORKSPACE="$U28_WS" \
DEBOPTY_SKIP_AGENT_WORKSPACE=1 \
_debopty_register_rust_binary "$U28_INST"
if grep -qF "agent init" "$U28_CALL_LOG"; then
    _fail "9i: DEBOPTY_SKIP_AGENT_WORKSPACE=1 should skip agent init"
else
    _pass "9i: DEBOPTY_SKIP_AGENT_WORKSPACE=1 skips agent init"
fi

if [[ "$FAILS" -ne 0 ]]; then
    echo "install-bootstrap-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "install-bootstrap-test: all passed"
