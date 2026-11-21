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
_assert_file_contains "${ROOT}/deb_optimizer.sh" "find \"\${INSTALL_DIR}\" -type f -name 'INDEX.md' -delete" "deb_optimizer.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${ROOT}/deb_optimizer.sh" 'VIBEINSTRCT.md' "deb_optimizer.sh removed VIBEINSTRCT.md typo"

_assert_file_contains "${ROOT}/install.sh" 'INSTALL_DIR="/opt/debopty"' "install.sh sets INSTALL_DIR=/opt/debopty"
_assert_file_contains "${ROOT}/install.sh" 'tools governance instructions tests' "install.sh strips governance in prod cleanup"
_assert_file_contains "${ROOT}/install.sh" "find \"\$INSTALL_DIR\" -type f -name 'INDEX.md' -delete" "install.sh strips INDEX.md routers in prod cleanup"
_assert_file_not_contains "${ROOT}/install.sh" 'VIBEINSTRCT.md' "install.sh has zero VIBEINSTRCT.md"

_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='governance'" "release.yml excludes governance from release tarball"
_assert_file_contains "${ROOT}/.github/workflows/release.yml" "--exclude='INDEX.md'" "release.yml excludes INDEX.md from release tarball"
_assert_file_contains "${ROOT}/tools/github-push.sh" "--exclude='governance'" "github-push.sh excludes governance from release tarball"
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

if [[ "$FAILS" -ne 0 ]]; then
    echo "install-bootstrap-test: ${FAILS} failure(s)" >&2
    exit 1
fi
echo "install-bootstrap-test: all passed"
