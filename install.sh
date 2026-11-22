#!/bin/bash
# =========================================================
# debopty (Debian Optimizer Script) 一键安装自举脚本 (Bootstrap)
# =========================================================
set -euo pipefail

_DEBOPTY_UA="Mozilla/5.0 (X11; Linux x86_64) debopty-bootstrap"
_DEBOPTY_LAST_FETCH_ERR=""
_DEBOPTY_LAST_FETCH_URL=""
_DEBOPTY_LAST_FETCH_ATTEMPTS=0
_DEBOPTY_LAST_SHA_ERR=""

_debopty_err_diag() {
    local stage=$1
    local summary=$2
    local detail=${3:-}
    local hint=${4:-}
    printf '\033[0;31m[错误] [stage=%s] %s\033[0m\n' "$stage" "$summary" >&2
    if [[ -n "$detail" ]]; then
        printf '  详情: %s\n' "$detail" >&2
    fi
    if [[ -n "$hint" ]]; then
        printf '  建议: %s\n' "$hint" >&2
    fi
}

# 零 curl/wget 兼容下载自举函数 (curl -> wget -> python3 -> perl -> apt-get)
_debopty_bootstrap_fetch() {
    local url=$1
    local dest=$2
    local sudo_prefix=()
    if [[ ${EUID:-$(id -u)} -ne 0 && "${DEBOPTY_NO_SUDO:-0}" != "1" ]] && command -v sudo >/dev/null 2>&1; then
        sudo_prefix=(sudo)
    fi

    local had_downloader="false"
    local need_ca_repair="false"
    local rc=0
    local err_out=""
    _DEBOPTY_LAST_FETCH_ERR=""

    rm -f "$dest"
    if command -v curl >/dev/null 2>&1; then
        had_downloader="true"
        rc=0
        err_out="$(curl -fsSL --connect-timeout 10 --max-time 120 -A "$_DEBOPTY_UA" "$url" -o "$dest" 2>&1)" || rc=$?
        if [[ "$rc" -eq 0 && -s "$dest" ]]; then
            _DEBOPTY_LAST_FETCH_ERR=""
            return 0
        fi
        rm -f "$dest"
        err_out="${err_out//$'\r'/}"
        err_out="${err_out//$'\n'/ | }"
        if [[ "$rc" -eq 0 ]]; then
            _DEBOPTY_LAST_FETCH_ERR="curl: empty response body"
        else
            _DEBOPTY_LAST_FETCH_ERR="curl(rc=${rc}): ${err_out:-request failed}"
            if [[ "$rc" -eq 22 || "$rc" -eq 37 ]]; then
                return 1
            fi
            if [[ "$rc" -eq 35 || "$rc" -eq 51 || "$rc" -eq 58 || "$rc" -eq 60 || "$rc" -eq 77 ]]; then
                need_ca_repair="true"
            fi
        fi
    fi

    if command -v wget >/dev/null 2>&1; then
        had_downloader="true"
        rc=0
        err_out="$(wget -qO "$dest" --timeout=30 --tries=2 --user-agent="$_DEBOPTY_UA" "$url" 2>&1)" || rc=$?
        if [[ "$rc" -eq 0 && -s "$dest" ]]; then
            _DEBOPTY_LAST_FETCH_ERR=""
            return 0
        fi
        rm -f "$dest"
        err_out="${err_out//$'\r'/}"
        err_out="${err_out//$'\n'/ | }"
        if [[ "$rc" -eq 0 ]]; then
            _DEBOPTY_LAST_FETCH_ERR="wget: empty response body"
        else
            _DEBOPTY_LAST_FETCH_ERR="wget(rc=${rc}): ${err_out:-request failed}"
            if [[ "$rc" -eq 8 ]]; then
                return 1
            fi
            if [[ "$rc" -eq 5 ]]; then
                need_ca_repair="true"
            fi
        fi
    fi

    if command -v python3 >/dev/null 2>&1; then
        had_downloader="true"
        rc=0
        err_out="$(python3 -c 'import shutil,socket,sys,urllib.request; socket.setdefaulttimeout(30); req=urllib.request.Request(sys.argv[1], headers={"User-Agent": sys.argv[2]}); r=urllib.request.urlopen(req, timeout=120); out=open(sys.argv[3], "wb"); shutil.copyfileobj(r, out); out.close(); r.close()' "$url" "$_DEBOPTY_UA" "$dest" 2>&1)" || rc=$?
        if [[ "$rc" -eq 0 && -s "$dest" ]]; then
            _DEBOPTY_LAST_FETCH_ERR=""
            return 0
        fi
        rm -f "$dest"
        err_out="${err_out//$'\r'/}"
        err_out="${err_out//$'\n'/ | }"
        if [[ "$rc" -eq 0 ]]; then
            _DEBOPTY_LAST_FETCH_ERR="python3: empty response body"
        else
            _DEBOPTY_LAST_FETCH_ERR="python3(rc=${rc}): ${err_out:-request failed}"
            if [[ "$err_out" == *"CERTIFICATE_VERIFY_FAILED"* ]]; then
                need_ca_repair="true"
            fi
        fi
    fi

    if command -v perl >/dev/null 2>&1; then
        had_downloader="true"
        rc=0
        err_out="$(perl -MHTTP::Tiny -e 'my $r=HTTP::Tiny->new(timeout=>30, agent=>$ARGV[0])->get($ARGV[1]); die "HTTP $r->{status} $r->{reason}\n" unless $r->{success}; open my $fh,">",$ARGV[2] or die "$!\n"; binmode $fh; print $fh $r->{content}' "$_DEBOPTY_UA" "$url" "$dest" 2>&1)" || rc=$?
        if [[ "$rc" -eq 0 && -s "$dest" ]]; then
            _DEBOPTY_LAST_FETCH_ERR=""
            return 0
        fi
        rm -f "$dest"
        err_out="${err_out//$'\r'/}"
        err_out="${err_out//$'\n'/ | }"
        if [[ "$rc" -eq 0 ]]; then
            _DEBOPTY_LAST_FETCH_ERR="perl: empty response body"
        else
            _DEBOPTY_LAST_FETCH_ERR="perl(rc=${rc}): ${err_out:-request failed}"
        fi
    fi

    if [[ "$had_downloader" == "false" || "$need_ca_repair" == "true" ]] && command -v apt-get >/dev/null 2>&1; then
        "${sudo_prefix[@]}" apt-get update -yq >/dev/null 2>&1 || true
        if ! "${sudo_prefix[@]}" apt-get install -yq ca-certificates curl >/dev/null 2>&1; then
            "${sudo_prefix[@]}" apt-get install -yq --fix-missing ca-certificates curl >/dev/null 2>&1 || true
        fi
        hash -r
        if command -v curl >/dev/null 2>&1; then
            rc=0
            err_out="$(curl -fsSL --connect-timeout 10 --max-time 120 -A "$_DEBOPTY_UA" "$url" -o "$dest" 2>&1)" || rc=$?
            if [[ "$rc" -eq 0 && -s "$dest" ]]; then
                _DEBOPTY_LAST_FETCH_ERR=""
                return 0
            fi
            rm -f "$dest"
            err_out="${err_out//$'\r'/}"
            err_out="${err_out//$'\n'/ | }"
            if [[ "$rc" -eq 0 ]]; then
                _DEBOPTY_LAST_FETCH_ERR="curl(post-apt): empty response body"
            else
                _DEBOPTY_LAST_FETCH_ERR="curl(post-apt,rc=${rc}): ${err_out:-request failed}"
            fi
        fi
    fi

    if [[ -z "$_DEBOPTY_LAST_FETCH_ERR" ]]; then
        _DEBOPTY_LAST_FETCH_ERR="no downloader available"
    fi
    rm -f "$dest"
    return 1
}

fetch_url() {
    local url=$1
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --connect-timeout 3 --max-time 6 "$url" 2>/dev/null && return 0
    fi
    if command -v wget >/dev/null 2>&1; then
        wget -qO- --timeout=5 --tries=1 "$url" 2>/dev/null && return 0
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import sys,urllib.request; print(urllib.request.urlopen(sys.argv[1], timeout=5).read().decode("utf-8", "ignore"))' "$url" 2>/dev/null && return 0
    fi
    if command -v perl >/dev/null 2>&1; then
        perl -MHTTP::Tiny -e 'my $r=HTTP::Tiny->new(timeout=>5)->get($ARGV[0]); exit 1 unless $r->{success}; print $r->{content}' "$url" 2>/dev/null && return 0
    fi
    return 0
}

_debopty_detect_arch() {
    local raw="${1:-$(uname -m)}"
    case "$raw" in
        x86_64|amd64) echo "amd64" ;;
        aarch64|arm64) echo "arm64" ;;
        *) return 1 ;;
    esac
}

_debopty_is_valid_semver_tag() {
    local tag=${1:-}
    local re_sv='^v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'
    [[ "$tag" =~ $re_sv ]]
}

_debopty_is_html_file() {
    local file=$1
    [[ -f "$file" && -s "$file" ]] || return 1
    local head_text="" line="" n=0
    while (( n < 12 )); do
        if ! IFS= read -r line; then
            [[ -n "$line" ]] && head_text+="$line"
            break
        fi
        head_text+="$line"$'\n'
        n=$((n + 1))
    done < "$file" 2>/dev/null || true

    head_text="${head_text#$'\xef\xbb\xbf'}"
    head_text="${head_text#"${head_text%%[![:space:]]*}"}"
    local lower="${head_text,,}"

    if [[ "$lower" == "<!doctype html"* || "$lower" == "<html"* || "$lower" == "<head"* || "$lower" == "<title"* || "$lower" == "<body"* ]]; then
        return 0
    fi
    if [[ "$lower" == "<!--"* ]] && [[ "$lower" == *"<!doctype html"* || "$lower" == *"<html"* ]]; then
        return 0
    fi
    return 1
}

_debopty_extract_api_tag() {
    local file=$1
    [[ -f "$file" && -s "$file" ]] || return 1
    if _debopty_is_html_file "$file"; then
        return 1
    fi
    local line="" stripped="" found_first="false"
    local re_api='"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)"'
    while IFS= read -r line || [[ -n "$line" ]]; do
        if [[ "$found_first" == "false" ]]; then
            stripped="${line#$'\xef\xbb\xbf'}"
            stripped="${stripped#"${stripped%%[![:space:]]*}"}"
            if [[ -n "$stripped" ]]; then
                found_first="true"
                if [[ "${stripped:0:1}" != "{" ]]; then
                    return 1
                fi
            fi
        fi
        if [[ "$line" =~ $re_api ]]; then
            local cand="${BASH_REMATCH[1]}"
            if _debopty_is_valid_semver_tag "$cand"; then
                printf '%s\n' "$cand"
                return 0
            fi
        fi
    done < "$file"
    return 1
}

_debopty_extract_release_page_tag() {
    local file=$1
    [[ -f "$file" && -s "$file" ]] || return 1
    local re_rel='releases/tag/([^"'\''<>[:space:]/#?]+)'
    local re_tag='tag/([^"'\''<>[:space:]/#?]+)'
    local line="" rest="" cand="" matched=""
    while IFS= read -r line || [[ -n "$line" ]]; do
        rest="$line"
        while [[ "$rest" =~ $re_rel ]]; do
            matched="${BASH_REMATCH[0]}"
            cand="${BASH_REMATCH[1]}"
            if _debopty_is_valid_semver_tag "$cand"; then
                printf '%s\n' "$cand"
                return 0
            fi
            rest="${rest#*"$matched"}"
        done
    done < "$file"
    while IFS= read -r line || [[ -n "$line" ]]; do
        rest="$line"
        while [[ "$rest" =~ $re_tag ]]; do
            matched="${BASH_REMATCH[0]}"
            cand="${BASH_REMATCH[1]}"
            if _debopty_is_valid_semver_tag "$cand"; then
                printf '%s\n' "$cand"
                return 0
            fi
            rest="${rest#*"$matched"}"
        done
    done < "$file"
    return 1
}

_debopty_validate_sha256sums() {
    local file=$1
    local asset_name=${2:-}
    [[ -f "$file" && -s "$file" ]] || return 1
    if _debopty_is_html_file "$file"; then
        return 1
    fi
    local line="" hash="" fname=""
    local re_hex='^[0-9a-fA-F]{64}$'
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%$'\r'}"
        [[ -z "$line" ]] && continue
        hash="${line%%[[:space:]]*}"
        fname="${line#"$hash"}"
        fname="${fname#"${fname%%[![:space:]]*}"}"
        fname="${fname#\*}"
        fname="${fname#./}"
        if [[ "$hash" =~ $re_hex && -n "$fname" ]]; then
            if [[ -z "$asset_name" || "$fname" == "$asset_name" ]]; then
                return 0
            fi
        fi
    done < "$file"
    return 1
}

_debopty_validate_targz() {
    local file=$1
    [[ -f "$file" && -s "$file" ]] || return 1
    if command -v tar >/dev/null 2>&1; then
        tar -tzf "$file" >/dev/null 2>&1
        return $?
    elif command -v gzip >/dev/null 2>&1; then
        gzip -t "$file" >/dev/null 2>&1
        return $?
    fi
    return 1
}

_debopty_validate_downloaded_asset() {
    local file=$1
    local expect_kind=${2:-non_html}
    local extra=${3:-}
    case "$expect_kind" in
        any)
            if [[ ! -f "$file" || ! -s "$file" ]]; then
                _DEBOPTY_LAST_FETCH_ERR="empty downloaded asset"
                return 1
            fi
            ;;
        non_html)
            if [[ ! -f "$file" || ! -s "$file" ]] || _debopty_is_html_file "$file"; then
                _DEBOPTY_LAST_FETCH_ERR="rejected soft-200 HTML response"
                return 1
            fi
            ;;
        api_json)
            if ! _debopty_extract_api_tag "$file" >/dev/null; then
                _DEBOPTY_LAST_FETCH_ERR="rejected non-JSON/HTML or invalid tag_name response"
                return 1
            fi
            ;;
        release_page)
            if ! _debopty_extract_release_page_tag "$file" >/dev/null; then
                _DEBOPTY_LAST_FETCH_ERR="rejected release page without valid SemVer tag"
                return 1
            fi
            ;;
        sha256sums)
            if ! _debopty_validate_sha256sums "$file" "$extra"; then
                _DEBOPTY_LAST_FETCH_ERR="rejected invalid/HTML SHA256SUMS response"
                return 1
            fi
            ;;
        targz)
            if ! _debopty_validate_targz "$file"; then
                _DEBOPTY_LAST_FETCH_ERR="rejected invalid/HTML tar.gz archive"
                return 1
            fi
            ;;
        *)
            if [[ ! -f "$file" || ! -s "$file" ]] || _debopty_is_html_file "$file"; then
                _DEBOPTY_LAST_FETCH_ERR="rejected soft-200 HTML response"
                return 1
            fi
            ;;
    esac
    return 0
}

_debopty_verify_sha256() {
    local file=$1
    local sums_file=$2
    local asset_name=$3
    _DEBOPTY_LAST_SHA_ERR=""
    if [[ ! -f "$file" || ! -s "$file" || ! -f "$sums_file" || ! -s "$sums_file" || -z "$asset_name" ]]; then
        _DEBOPTY_LAST_SHA_ERR="校验输入文件或 SHA256SUMS 缺失"
        return 1
    fi

    local expected="" line hash fname
    while IFS= read -r line || [[ -n "$line" ]]; do
        line="${line%$'\r'}"
        [[ -z "$line" ]] && continue
        hash="${line%%[[:space:]]*}"
        fname="${line#"$hash"}"
        fname="${fname#"${fname%%[![:space:]]*}"}"
        fname="${fname#\*}"
        fname="${fname#./}"
        if [[ "$fname" == "$asset_name" && "$hash" =~ ^[0-9a-fA-F]{64}$ ]]; then
            expected="${hash,,}"
            break
        fi
    done < "$sums_file"

    if [[ -z "$expected" ]]; then
        _DEBOPTY_LAST_SHA_ERR="SHA256SUMS 中缺少 ${asset_name} 的有效 64 位摘要"
        return 1
    fi

    local actual="" out=""
    if command -v sha256sum >/dev/null 2>&1; then
        out="$(sha256sum "$file" 2>/dev/null || true)"
        actual="${out%%[[:space:]]*}"
    elif command -v openssl >/dev/null 2>&1; then
        out="$(openssl dgst -sha256 "$file" 2>/dev/null || true)"
        actual="${out##* }"
    elif command -v python3 >/dev/null 2>&1; then
        actual="$(python3 -c 'import hashlib,sys; print(hashlib.sha256(open(sys.argv[1],"rb").read()).hexdigest())' "$file" 2>/dev/null || true)"
    else
        _DEBOPTY_LAST_SHA_ERR="未找到 sha256sum / openssl / python3 摘要校验工具"
        return 1
    fi

    actual="${actual,,}"
    if [[ -n "$actual" && "$actual" == "$expected" ]]; then
        _DEBOPTY_LAST_SHA_ERR=""
        return 0
    fi
    _DEBOPTY_LAST_SHA_ERR="expected=${expected} actual=${actual:-<empty>}"
    return 1
}

_debopty_fetch_release_asset() {
    local is_cn=$1
    local url=$2
    local dest=$3
    local expect_kind=${4:-non_html}
    local extra=${5:-}

    _DEBOPTY_LAST_FETCH_ERR=""
    _DEBOPTY_LAST_FETCH_URL=""
    _DEBOPTY_LAST_FETCH_ATTEMPTS=0

    local re_gh_host='^https?://([^/@]+@)?([^/:]+\.)?(github\.com|githubusercontent\.com)(:|/|$)'
    if [[ "$is_cn" == "true" ]] && [[ "$url" =~ $re_gh_host ]]; then
        local mirrors=()
        if [[ "$expect_kind" == "api_json" || "$expect_kind" == "release_page" ]]; then
            mirrors=(
                "prefix|https://gh-proxy.com"
                "prefix|https://edgeone.gh-proxy.org"
                "prefix|https://gh-proxy.org"
                "prefix|https://cdn.gh-proxy.org"
                "prefix|https://git.yylx.win"
                "prefix|https://wget.la"
                "direct|"
            )
        else
            mirrors=(
                "prefix|https://gh-proxy.com"
                "prefix|https://cdn.gh-proxy.org"
                "prefix|https://edgeone.gh-proxy.org"
                "prefix|https://git.yylx.win"
                "prefix|https://gh-proxy.org"
                "jsdelivr|"
                "prefix|https://ghfast.top"
                "prefix|https://wget.la"
                "direct|"
            )
        fi
        local mirror_conf mode rest dl_url
        for mirror_conf in "${mirrors[@]}"; do
            mode="${mirror_conf%%|*}"
            rest="${mirror_conf#*|}"
            dl_url=""
            if [[ "$mode" == "jsdelivr" ]]; then
                if [[ "$url" =~ ^https://raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.*)$ ]]; then
                    dl_url="https://cdn.jsdelivr.net/gh/${BASH_REMATCH[1]}/${BASH_REMATCH[2]}@${BASH_REMATCH[3]}/${BASH_REMATCH[4]}"
                else
                    continue
                fi
            elif [[ "$mode" == "prefix" ]]; then
                if [[ "$rest" == "https://ghfast.top" ]] && [[ "$url" =~ api\.github\.com|/releases/latest([?#].*)?$ ]]; then
                    continue
                fi
                dl_url="${rest}/${url}"
            elif [[ "$mode" == "direct" ]]; then
                dl_url="$url"
            fi

            _DEBOPTY_LAST_FETCH_ATTEMPTS=$((_DEBOPTY_LAST_FETCH_ATTEMPTS + 1))
            _DEBOPTY_LAST_FETCH_URL="$dl_url"
            if _debopty_bootstrap_fetch "$dl_url" "$dest"; then
                if _debopty_validate_downloaded_asset "$dest" "$expect_kind" "$extra"; then
                    _DEBOPTY_LAST_FETCH_ERR=""
                    return 0
                fi
                rm -f "$dest"
                _DEBOPTY_LAST_FETCH_ERR="${_DEBOPTY_LAST_FETCH_ERR} (${dl_url})"
            fi
        done
        return 1
    fi

    _DEBOPTY_LAST_FETCH_ATTEMPTS=1
    _DEBOPTY_LAST_FETCH_URL="$url"
    if _debopty_bootstrap_fetch "$url" "$dest"; then
        if _debopty_validate_downloaded_asset "$dest" "$expect_kind" "$extra"; then
            _DEBOPTY_LAST_FETCH_ERR=""
            return 0
        fi
        rm -f "$dest"
        _DEBOPTY_LAST_FETCH_ERR="${_DEBOPTY_LAST_FETCH_ERR} (${url})"
    fi
    return 1
}

_debopty_build_from_source() {
    local install_dir=$1
    if [[ ! -f "${install_dir}/rust/Cargo.toml" ]]; then
        _debopty_err_diag "source-fallback" "源码编译回退不可用：未找到 ${install_dir}/rust/Cargo.toml"
        return 1
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        _debopty_err_diag "source-fallback" "源码编译回退不可用：系统未安装 cargo" \
            "" "请安装 Rust 工具链后重试，或检查网络以下载预编译二进制。"
        return 1
    fi

    printf '\033[1;33m[提示] 正在通过 DEBOPTY_SOURCE_FALLBACK=1 从源码构建原生 Rust 二进制...\033[0m\n' >&2
    local build_rc=0
    (
        cd "${install_dir}/rust"
        cargo build --release --locked --features ratatui-ui
    ) || build_rc=$?
    if [[ "$build_rc" -ne 0 ]]; then
        _debopty_err_diag "source-fallback" "cargo 源码构建失败 (rc=${build_rc})" \
            "manifest=${install_dir}/rust/Cargo.toml"
        return 1
    fi

    local built_bin="${install_dir}/rust/target/release/debopty"
    if [[ ! -f "$built_bin" || ! -s "$built_bin" ]]; then
        _debopty_err_diag "source-fallback" "源码构建产物缺失或为空" "path=${built_bin}"
        return 1
    fi

    mkdir -p "${install_dir}/bin"
    local staged_bin="${install_dir}/bin/.debopty.new.$$"
    cp "$built_bin" "$staged_bin"
    chmod 0755 "$staged_bin"
    mv -f "$staged_bin" "${install_dir}/bin/debopty"
    return 0
}

_debopty_prepare_rust_binary() {
    local install_dir=$1
    local is_cn=${2:-false}
    local tag=${3:-${DEBOPTY_RELEASE_TAG:-}}
    local mode="${DEBOPTY_INSTALL_BINARY:-auto}"

    if [[ "$mode" == "0" || "$mode" == "false" ]]; then
        return 0
    fi

    local raw_arch arch
    raw_arch="$(uname -m 2>/dev/null || true)"
    if ! arch="$(_debopty_detect_arch "$raw_arch")"; then
        _debopty_err_diag "arch" "不支持的系统架构: ${raw_arch:-unknown}" \
            "仅支持 amd64 (x86_64) 与 arm64 (aarch64)" \
            "若需自行编译，可设置 DEBOPTY_SOURCE_FALLBACK=1（需 cargo）"
        if [[ "${DEBOPTY_SOURCE_FALLBACK:-0}" == "1" ]]; then
            _debopty_build_from_source "$install_dir"
            return $?
        fi
        return 1
    fi

    local repo="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"
    local tmp_dir
    if ! tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/debopty-bin.XXXXXX")"; then
        _debopty_err_diag "mktemp" "无法创建临时工作目录" "TMPDIR=${TMPDIR:-/tmp}"
        return 1
    fi

    local tag_candidates=()
    local api_err="" page_err="" version_status="missing"
    if [[ -n "$tag" ]]; then
        local norm_tag="${tag#v}"
        if [[ "$norm_tag" =~ ^[0-9] ]]; then
            norm_tag="v${norm_tag}"
        else
            norm_tag="$tag"
        fi
        tag_candidates=("${norm_tag}|explicit")
    else
        local version_tag=""
        if [[ -f "${install_dir}/VERSION" ]]; then
            version_status="invalid"
            local vline="" raw_sv=""
            local re_ver='^DEBOPTI_SEMVER=["'\'']?([^"'\'']+)["'\'']?'
            while IFS= read -r vline || [[ -n "$vline" ]]; do
                vline="${vline%$'\r'}"
                if [[ "$vline" =~ $re_ver ]]; then
                    raw_sv="${BASH_REMATCH[1]#v}"
                    if _debopty_is_valid_semver_tag "v${raw_sv}"; then
                        version_tag="v${raw_sv}"
                        version_status="ok(${version_tag})"
                        break
                    fi
                fi
            done < "${install_dir}/VERSION"
        fi

        if [[ -z "${DEBOPTY_RELEASE_BASE_URL:-}" ]]; then
            local remote_tag="" remote_src=""
            local latest_json="${tmp_dir}/latest.json"
            if _debopty_fetch_release_asset "$is_cn" "https://api.github.com/repos/${repo}/releases/latest" "$latest_json" "api_json"; then
                remote_tag="$(_debopty_extract_api_tag "$latest_json" || true)"
                remote_src="github-api"
            else
                api_err="${_DEBOPTY_LAST_FETCH_ERR:-fetch failed}"
            fi

            if [[ -z "$remote_tag" ]]; then
                local latest_html="${tmp_dir}/latest.html"
                if _debopty_fetch_release_asset "$is_cn" "https://github.com/${repo}/releases/latest" "$latest_html" "release_page"; then
                    remote_tag="$(_debopty_extract_release_page_tag "$latest_html" || true)"
                    remote_src="github-releases-page"
                else
                    page_err="${_DEBOPTY_LAST_FETCH_ERR:-fetch failed}"
                fi
            fi

            if [[ -n "$remote_tag" ]]; then
                local norm_remote="${remote_tag#v}"
                if [[ "$norm_remote" =~ ^[0-9] ]]; then
                    remote_tag="v${norm_remote}"
                fi
                tag_candidates+=("${remote_tag}|${remote_src}")
            fi
            if [[ -n "$version_tag" && "$version_tag" != "$remote_tag" ]]; then
                tag_candidates+=("${version_tag}|VERSION-file")
            fi
        else
            if [[ -n "$version_tag" ]]; then
                tag_candidates+=("${version_tag}|VERSION-file")
            fi
        fi
    fi

    if [[ "${#tag_candidates[@]}" -eq 0 ]]; then
        _debopty_err_diag "tag" "无法解析有效的 SemVer Release 标签" \
            "api=${api_err:-skipped}; releases_page=${page_err:-skipped}; VERSION=${version_status}" \
            "请检查网络连接，或通过 DEBOPTY_RELEASE_TAG=vX.Y.Z 显式指定版本。"
        rm -rf "$tmp_dir"
        if [[ "${DEBOPTY_SOURCE_FALLBACK:-0}" == "1" ]]; then
            _debopty_build_from_source "$install_dir"
            return $?
        fi
        return 1
    fi

    local idx=0
    local total="${#tag_candidates[@]}"
    local entry cand_tag cand_src asset_name base_url sums_file archive_file
    for entry in "${tag_candidates[@]}"; do
        cand_tag="${entry%%|*}"
        cand_src="${entry#*|}"
        asset_name="debopty-${cand_tag}-linux-${arch}.tar.gz"
        if [[ -n "${DEBOPTY_RELEASE_BASE_URL:-}" ]]; then
            base_url="${DEBOPTY_RELEASE_BASE_URL%/}"
        else
            base_url="https://github.com/${repo}/releases/download/${cand_tag}"
        fi

        sums_file="${tmp_dir}/SHA256SUMS"
        archive_file="${tmp_dir}/${asset_name}"
        rm -rf "$sums_file" "$archive_file" "${tmp_dir}/extract"

        if ! _debopty_fetch_release_asset "$is_cn" "${base_url}/SHA256SUMS" "$sums_file" "sha256sums"; then
            _debopty_err_diag "sha256sums" "下载或校验 SHA256SUMS 失败" \
                "tag=${cand_tag} source=${cand_src} arch=${arch} attempts=${_DEBOPTY_LAST_FETCH_ATTEMPTS} url=${_DEBOPTY_LAST_FETCH_URL} err=${_DEBOPTY_LAST_FETCH_ERR}" \
                "请检查网络/镜像连通性，或通过 DEBOPTY_RELEASE_TAG 指定可用版本。"
            if (( idx + 1 < total )); then
                printf '\033[1;33m[提示] 标签 %s 不可用，正在尝试下一候选标签...\033[0m\n' "$cand_tag" >&2
                idx=$((idx + 1))
                continue
            fi
            break
        fi

        if ! _debopty_validate_sha256sums "$sums_file" "$asset_name"; then
            _debopty_err_diag "sha256sums-entry" "SHA256SUMS 中缺少目标架构资产条目" \
                "tag=${cand_tag} source=${cand_src} arch=${arch} asset=${asset_name}" \
                "该 Release 可能未发布当前架构 (${arch}) 的预编译包，可尝试其他版本。"
            if (( idx + 1 < total )); then
                printf '\033[1;33m[提示] 标签 %s 缺少 %s，正在尝试下一候选标签...\033[0m\n' "$cand_tag" "$asset_name" >&2
                idx=$((idx + 1))
                continue
            fi
            break
        fi

        if ! _debopty_fetch_release_asset "$is_cn" "${base_url}/${asset_name}" "$archive_file" "targz"; then
            _debopty_err_diag "archive" "下载或校验二进制归档失败" \
                "tag=${cand_tag} source=${cand_src} arch=${arch} attempts=${_DEBOPTY_LAST_FETCH_ATTEMPTS} url=${_DEBOPTY_LAST_FETCH_URL} err=${_DEBOPTY_LAST_FETCH_ERR}" \
                "请检查网络连接与镜像节点，或重试安装。"
            if (( idx + 1 < total )); then
                printf '\033[1;33m[提示] 标签 %s 归档下载失败，正在尝试下一候选标签...\033[0m\n' "$cand_tag" >&2
                idx=$((idx + 1))
                continue
            fi
            break
        fi

        if ! _debopty_verify_sha256 "$archive_file" "$sums_file" "$asset_name"; then
            _debopty_err_diag "sha256" "二进制归档 SHA-256 摘要校验失败（已安全中止）" \
                "tag=${cand_tag} source=${cand_src} asset=${asset_name} ${_DEBOPTY_LAST_SHA_ERR}" \
                "下载文件可能损坏或被篡改，请更换网络环境后重试。"
            rm -rf "$tmp_dir"
            return 1
        fi

        mkdir -p "${tmp_dir}/extract"
        local tar_err=""
        if ! tar_err="$(tar -xzf "$archive_file" -C "${tmp_dir}/extract" 2>&1)"; then
            tar_err="${tar_err//$'\r'/}"
            tar_err="${tar_err//$'\n'/ | }"
            _debopty_err_diag "extract" "解压二进制归档失败" \
                "tag=${cand_tag} asset=${asset_name} err=${tar_err:-tar failed}"
            rm -rf "$tmp_dir"
            return 1
        fi

        if [[ ! -f "${tmp_dir}/extract/debopty" || ! -s "${tmp_dir}/extract/debopty" ]]; then
            _debopty_err_diag "binary" "归档中缺少有效的 debopty 可执行文件" \
                "tag=${cand_tag} asset=${asset_name}"
            rm -rf "$tmp_dir"
            return 1
        fi

        mkdir -p "${install_dir}/bin"
        # BE-P1-15: stage sibling file + chmod + atomic mv to prevent ETXTBSY when upgrading a running binary
        local staged_bin="${install_dir}/bin/.debopty.new.$$"
        cp "${tmp_dir}/extract/debopty" "$staged_bin"
        chmod 0755 "$staged_bin"
        mv -f "$staged_bin" "${install_dir}/bin/debopty"

        rm -rf "$tmp_dir"
        return 0
    done

    rm -rf "$tmp_dir"
    if [[ "${DEBOPTY_SOURCE_FALLBACK:-0}" == "1" ]]; then
        _debopty_build_from_source "$install_dir"
        return $?
    fi
    if command -v cargo >/dev/null 2>&1 && [[ -f "${install_dir}/rust/Cargo.toml" ]]; then
        printf '\033[1;33m[提示] 检测到本机已安装 cargo，如需从源码编译回退，请设置 DEBOPTY_SOURCE_FALLBACK=1 后重试。\033[0m\n' >&2
    fi
    return 1
}

_debopty_register_rust_binary() {
    local install_dir=$1

    local bin_link="${DEBOPTY_BIN_LINK:-/usr/local/bin/debopty}"
    local f2b_link="${DEBOPTY_F2B_BIN_LINK:-/usr/local/bin/debopty-f2b-sync}"
    local rs_link="${DEBOPTY_RS_BIN_LINK:-/usr/local/bin/debopty-rs}"
    mkdir -p "$(dirname "$bin_link")" "$(dirname "$f2b_link")" "$(dirname "$rs_link")" 2>/dev/null || true
    ln -sf "${install_dir}/bin/debopty" "$bin_link" 2>/dev/null || true
    ln -sf "${install_dir}/bin/debopty" "$f2b_link" 2>/dev/null || true
    ln -sf "${install_dir}/bin/debopty" "$rs_link" 2>/dev/null || true

    # 迁移并清理旧版 debopti 快捷入口、安装目录与 Shell 注入（保留 /etc/debopti 等兼容状态）
    local legacy_link="${DEBOPTY_LEGACY_BIN_LINK:-/usr/local/bin/debopti}"
    local legacy_install_dir="${DEBOPTY_LEGACY_INSTALL_DIR:-/opt/debopti}"
    local profile_dir="${DEBOPTY_PROFILE_DIR:-/etc/profile.d}"
    local fish_conf_dir="${DEBOPTY_FISH_CONF_DIR:-/etc/fish/conf.d}"

    if [[ -L "$legacy_link" ]]; then
        local legacy_target="" legacy_raw=""
        legacy_target="$(readlink -f "$legacy_link" 2>/dev/null || true)"
        legacy_raw="$(readlink "$legacy_link" 2>/dev/null || true)"
        if [[ "$legacy_target" == *"deb_optimizer.sh"* || "$legacy_target" == *"debopti"* || "$legacy_target" == *"debopty"* || \
              "$legacy_raw" == *"deb_optimizer.sh"* || "$legacy_raw" == *"debopti"* || "$legacy_raw" == *"debopty"* || \
              ! -e "$legacy_link" ]]; then
            rm -f "$legacy_link" 2>/dev/null || true
        fi
    elif [[ -f "$legacy_link" ]]; then
        local lline="" ln_idx=0 is_legacy_wrapper="false"
        while (( ln_idx < 64 )); do
            if ! IFS= read -r lline; then
                if [[ "$lline" == *"deb_optimizer.sh"* || "$lline" == *"/opt/debopti"* || "$lline" == *"/opt/debopty"* ]]; then
                    is_legacy_wrapper="true"
                fi
                break
            fi
            if [[ "$lline" == *"deb_optimizer.sh"* || "$lline" == *"/opt/debopti"* || "$lline" == *"/opt/debopty"* ]]; then
                is_legacy_wrapper="true"
                break
            fi
            ln_idx=$((ln_idx + 1))
        done < "$legacy_link" 2>/dev/null || true
        if [[ "$is_legacy_wrapper" == "true" ]]; then
            rm -f "$legacy_link" 2>/dev/null || true
        fi
    fi

    rm -f "${profile_dir}/debopti.sh" "${fish_conf_dir}/debopti.fish" 2>/dev/null || true

    if [[ -n "$legacy_install_dir" && "$legacy_install_dir" != "/" && "$legacy_install_dir" != "/opt" && "$legacy_install_dir" != "$install_dir" ]]; then
        if [[ -L "$legacy_install_dir" ]]; then
            local lid_target="" lid_raw=""
            lid_target="$(readlink -f "$legacy_install_dir" 2>/dev/null || true)"
            lid_raw="$(readlink "$legacy_install_dir" 2>/dev/null || true)"
            if [[ "$lid_target" == *"debopti"* || "$lid_target" == *"debopty"* || \
                  "$lid_raw" == *"debopti"* || "$lid_raw" == *"debopty"* || \
                  ! -e "$legacy_install_dir" ]]; then
                rm -f "$legacy_install_dir" 2>/dev/null || true
            fi
        elif [[ -d "$legacy_install_dir" ]]; then
            if [[ -f "${legacy_install_dir}/deb_optimizer.sh" || -f "${legacy_install_dir}/VERSION" || -f "${legacy_install_dir}/scripts/common.sh" ]]; then
                rm -rf "$legacy_install_dir" 2>/dev/null || true
            fi
        fi
    fi

    if [[ "$legacy_install_dir" == "/opt/debopti" ]] && command -v fish >/dev/null 2>&1; then
        # shellcheck disable=SC2016
        fish -c 'if set -l idx (contains -i -- /opt/debopti $fish_user_paths); set -e fish_user_paths[$idx]; end' >/dev/null 2>&1 || true
    fi

    # 使用原生二进制渲染全局 Shell 快捷命令模板
    if [[ -w "$profile_dir" ]] || mkdir -p "$profile_dir" 2>/dev/null; then
        if "${install_dir}/bin/debopty" template render "templates/common/debopty.sh" \
            --out "${profile_dir}/debopty.sh" --var "BIN_LINK=${bin_link}" >/dev/null 2>&1; then
            chmod +x "${profile_dir}/debopty.sh" 2>/dev/null || true
        fi
    fi
    if command -v fish >/dev/null 2>&1 || [[ -d "$fish_conf_dir" ]]; then
        if [[ -w "$fish_conf_dir" ]] || mkdir -p "$fish_conf_dir" 2>/dev/null; then
            "${install_dir}/bin/debopty" template render "templates/common/debopty.fish" \
                --out "${fish_conf_dir}/debopty.fish" --var "BIN_LINK=${bin_link}" >/dev/null 2>&1 || true
        fi
    fi
    return 0
}

_debopty_install_rust_binary() {
    local install_dir=$1
    local is_cn=${2:-false}
    local tag=${3:-${DEBOPTY_RELEASE_TAG:-}}
    local mode="${DEBOPTY_INSTALL_BINARY:-auto}"

    if [[ "$mode" == "0" || "$mode" == "false" ]]; then
        return 0
    fi

    if ! _debopty_prepare_rust_binary "$install_dir" "$is_cn" "$tag"; then
        return 1
    fi
    _debopty_register_rust_binary "$install_dir"
}

_debopty_resolve_repo() {
    printf '%s\n' "${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"
}

_debopty_resolve_install_url() {
    local repo
    repo="$(_debopty_resolve_repo)"
    printf '%s\n' "${DEBOPTY_INSTALL_URL:-https://raw.githubusercontent.com/${repo}/main/install.sh}"
}

_debopty_resolve_repo_url() {
    local repo
    repo="$(_debopty_resolve_repo)"
    printf 'https://github.com/%s.git\n' "$repo"
}

# BE-P1-18: symlink-safe temporary DNS fallback (handles systemd-resolved/dangling symlinks)
_debopty_write_temp_dns() {
    local resolv_path="${1:-/etc/resolv.conf}"
    local parent_dir
    parent_dir="$(dirname "$resolv_path")"
    mkdir -p "$parent_dir"

    if [[ -L "$resolv_path" ]]; then
        if [[ ! -f "${resolv_path}.debopti.symlink.bak" ]]; then
            readlink "$resolv_path" > "${resolv_path}.debopti.symlink.bak" 2>/dev/null || true
        fi
        rm -f "$resolv_path"
    elif [[ -f "$resolv_path" && ! -f "${resolv_path}.debopti.bak" ]]; then
        cp "$resolv_path" "${resolv_path}.debopti.bak"
    fi

    local tmp_resolv="${resolv_path}.debopty.tmp.$$"
    cat > "$tmp_resolv" << 'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
EOF
    chmod 0644 "$tmp_resolv" 2>/dev/null || true
    mv -f "$tmp_resolv" "$resolv_path"
}

_debopty_stage_clone_dir() {
    local staging_dir=$1
    shift
    local parent_dir
    parent_dir="$(dirname "$staging_dir")"
    mkdir -p "$parent_dir"

    local cloned="false"
    local candidate_url
    for candidate_url in "$@"; do
        rm -rf "$staging_dir"
        if git clone "$candidate_url" "$staging_dir"; then
            cloned="true"
            break
        fi
        echo -e "\033[1;33m[提示] 节点拉取失败，尝试下一加速镜像...\033[0m"
    done

    if [[ "$cloned" != "true" ]]; then
        rm -rf "$staging_dir"
        return 1
    fi
    return 0
}

_debopty_swap_staged_dir() {
    local staging_dir=$1
    local install_dir=$2
    local old_dir="${install_dir}.old.$$"
    local parent_dir
    parent_dir="$(dirname "$install_dir")"
    mkdir -p "$parent_dir"

    rm -rf "$old_dir"
    if [[ -e "$install_dir" || -L "$install_dir" ]]; then
        mv "$install_dir" "$old_dir"
    fi
    if ! mv "$staging_dir" "$install_dir"; then
        if [[ -e "$old_dir" || -L "$old_dir" ]]; then
            mv "$old_dir" "$install_dir" || true
        fi
        rm -rf "$staging_dir"
        return 1
    fi
    rm -rf "$old_dir"
    return 0
}

# BE-P1-18: non-destructive staged git clone; preserves existing $install_dir if all mirrors fail
_debopty_clone_install_dir() {
    local install_dir=$1
    shift
    local staging_dir="${install_dir}.staging.$$"
    if ! _debopty_stage_clone_dir "$staging_dir" "$@"; then
        return 1
    fi
    _debopty_swap_staged_dir "$staging_dir" "$install_dir"
}

_debopty_strip_dev_metadata() {
    local INSTALL_DIR=$1
    (
        cd "$INSTALL_DIR"
        rm -rf .git .jj .github .jj_backup .cursor tools governance instructions tests rust archive AGENTS.md CLAUDE.md GEMINI.md Makefile .shellcheckrc .editorconfig .gitignore
    )
    find "$INSTALL_DIR" -type f -name 'INDEX.md' -delete 2>/dev/null || true
}

_debopty_upgrade_install_dir() {
    local install_dir=$1
    local is_cn=$2
    shift 2
    local staging_dir="${install_dir}.staging.$$"
    local legacy_install_dir="${DEBOPTY_LEGACY_INSTALL_DIR:-/opt/debopti}"

    local preserved_dir=""
    if [[ -e "$install_dir" || -L "$install_dir" ]]; then
        preserved_dir="$install_dir"
    elif [[ -e "$legacy_install_dir" || -L "$legacy_install_dir" ]]; then
        preserved_dir="$legacy_install_dir"
    fi

    if ! _debopty_stage_clone_dir "$staging_dir" "$@"; then
        rm -rf "$staging_dir"
        if [[ -n "$preserved_dir" ]]; then
            printf '\033[1;33m[提示] 已保留原有 %s 安装，未做破坏性变更。\033[0m\n' "$preserved_dir" >&2
        fi
        return 1
    fi

    if ! _debopty_prepare_rust_binary "$staging_dir" "$is_cn"; then
        rm -rf "$staging_dir"
        if [[ -n "$preserved_dir" ]]; then
            printf '\033[1;33m[提示] 已保留原有 %s 安装，未做破坏性变更。\033[0m\n' "$preserved_dir" >&2
        fi
        return 1
    fi

    _debopty_strip_dev_metadata "$staging_dir"

    if ! _debopty_swap_staged_dir "$staging_dir" "$install_dir"; then
        rm -rf "$staging_dir"
        return 1
    fi

    if [[ -x "${install_dir}/bin/debopty" ]]; then
        _debopty_register_rust_binary "$install_dir"
    fi
    return 0
}

main() {
    local repo install_url
    repo="$(_debopty_resolve_repo)"
    install_url="$(_debopty_resolve_install_url)"

    # 1. 权限拦截与自动提权
    if [[ $EUID -ne 0 ]]; then
        if command -v sudo >/dev/null 2>&1; then
            echo -e "\033[1;33m⚠️  当前非 root 权限，正在尝试通过 sudo 自动提权...\033[0m"
            if [[ -f "$0" ]]; then
                exec sudo bash "$0" "$@"
            else
                local tmp_script
                tmp_script="$(mktemp /tmp/debopty-install.XXXXXX.sh)"
                if _debopty_bootstrap_fetch "$install_url" "$tmp_script"; then
                    chmod +x "$tmp_script"
                    exec sudo bash "$tmp_script" "$@"
                else
                    rm -f "$tmp_script"
                    echo -e "\033[0;31m[错误] 权限不足且无法下载自举脚本，请使用 root 用户手动执行。\033[0m"
                    exit 1
                fi
            fi
        else
            echo -e "\033[0;31m[错误] 权限不足，且系统未安装 sudo。\033[0m"
            echo -e "\033[1;32m请使用以下方法之一运行：\033[0m"
            echo -e "  1. 切换到 root 用户运行（零依赖一键命令，支持无 curl/wget）："
            echo -e "     \033[1;36msu - -c 'u=\"${install_url}\"; f=\$(mktemp); trap \"rm -f \\\"\$f\\\"\" EXIT; curl -fsSL \"\$u\" -o \"\$f\" 2>/dev/null || wget -qO \"\$f\" \"\$u\" 2>/dev/null || python3 -c \"import socket,sys,urllib.request;socket.setdefaulttimeout(30);urllib.request.urlretrieve(sys.argv[1],sys.argv[2])\" \"\$u\" \"\$f\" 2>/dev/null || { apt-get update -yq && apt-get install -yq ca-certificates curl && curl -fsSL \"\$u\" -o \"\$f\"; }; bash \"\$f\"'\033[0m"
            echo -e "  2. 安装 sudo 并将当前用户加入 sudoers 组后再试。"
            exit 1
        fi
    fi

    # 2. 基础环境自举 (补齐 ca-certificates/curl/git/wget)
    echo -e "\033[0;32m[1/4] 正在检查基础环境...\033[0m"

    # 临时 DNS 修复逻辑：若当前 DNS 无法解析 github 域名，写入临时 DNS (1.1.1.1/8.8.8.8) 恢复连接
    if ! getent hosts raw.githubusercontent.com >/dev/null 2>&1 && ! getent hosts github.com >/dev/null 2>&1; then
        echo -e "\033[1;33m⚠️  检测到 DNS 解析故障，正在写入临时公共 DNS (1.1.1.1/8.8.8.8) 恢复连接...\033[0m"
        _debopty_write_temp_dns /etc/resolv.conf
    fi

    # 智能补齐基础工具
    if ! dpkg -s ca-certificates >/dev/null 2>&1 || ! command -v curl >/dev/null 2>&1 || ! command -v git >/dev/null 2>&1 || ! command -v wget >/dev/null 2>&1; then
        echo -e "\033[0;32m检测到系统缺少基础工具 (ca-certificates/curl/git/wget)，正在安装...\033[0m"
        apt-get update -yq >/dev/null 2>&1 || true
        if ! apt-get install -yq ca-certificates curl git wget >/dev/null 2>&1; then
            echo -e "\033[1;33m⚠️  标准安装失败，尝试通过 --fix-missing 修复安装...\033[0m"
            apt-get install -yq --fix-missing ca-certificates curl git wget >/dev/null 2>&1 || {
                echo -e "\033[0;31m❌ 基础工具 (ca-certificates/curl/git/wget) 安装失败，请检查您的网络连接与软件源配置。\033[0m"
                exit 1
            }
        fi
    else
        echo -e "\033[0;32m基础工具 (ca-certificates/curl/git/wget) 已就绪，跳过安装阶段。\033[0m"
    fi

    # 3. 归属地探测 (智能路由选择)
    echo -e "\033[0;32m[2/4] 正在检测网络环境...\033[0m"
    IS_CN=false
    local geo_country="" geo_ipip="" geo_cip=""
    geo_country="$(fetch_url https://ipinfo.io/country)"
    geo_country="${geo_country//[[:space:]]/}"
    if [[ "$geo_country" == "CN" ]]; then
        IS_CN=true
    else
        geo_ipip="$(fetch_url http://myip.ipip.net)"
        if [[ "$geo_ipip" == *"中国"* ]]; then
            IS_CN=true
        else
            geo_cip="$(fetch_url http://cip.cc)"
            if [[ "$geo_cip" == *"中国"* ]]; then
                IS_CN=true
            elif [[ -z "$geo_country" && -z "$geo_ipip" && -z "$geo_cip" ]]; then
                local cfg_file="${DEBOPTY_CONFIG_FILE:-/etc/debopti/debopti.conf}"
                if [[ -f "$cfg_file" ]]; then
                    local cline=""
                    local re_cn='^IS_CN_REGION=["'\'']?true["'\'']?[[:space:]]*$'
                    while IFS= read -r cline || [[ -n "$cline" ]]; do
                        cline="${cline%$'\r'}"
                        if [[ "$cline" =~ $re_cn ]]; then
                            IS_CN=true
                            break
                        fi
                    done < "$cfg_file"
                fi
            fi
        fi
    fi

    # 4. 事务型暂存升级：先拉取至暂存目录并校验 Rust 二进制，全部成功后再原子切换
    INSTALL_DIR="/opt/debopty"
    REPO_URL="$(_debopty_resolve_repo_url)"
    local clone_candidates=("$REPO_URL")

    if [[ "$IS_CN" == "true" ]]; then
        echo -e "\033[1;33m[提示] 检测到中国大陆环境，已启用 GitHub 加速镜像轮询 (ghfast.top -> gh-proxy.org -> wget.la -> gh-proxy.com)。\033[0m"
        clone_candidates=(
            "https://ghfast.top/${REPO_URL}"
            "https://gh-proxy.org/${REPO_URL}"
            "https://wget.la/${REPO_URL}"
            "https://gh-proxy.com/${REPO_URL}"
            "${REPO_URL}"
        )
    fi

    echo -e "\033[0;32m[3/4] 正在拉取项目资产至暂存目录并准备升级 $INSTALL_DIR (${repo}) ...\033[0m"
    echo -e "\033[0;32m[4/4] 正在校验并原子切换原生 Rust 引擎...\033[0m"
    if ! _debopty_upgrade_install_dir "$INSTALL_DIR" "$IS_CN" "${clone_candidates[@]}"; then
        echo -e "\033[0;31m[错误] 安装或升级失败，请查看上方阶段诊断信息后重试（可通过 DEBOPTY_RELEASE_TAG 指定版本或 DEBOPTY_SOURCE_FALLBACK=1 启用源码编译回退）。\033[0m" >&2
        exit 1
    fi

    # 5. 移交控制权 (Pure Rust 二进制默认入口)
    cd "$INSTALL_DIR"
    if [[ -z "${HOME:-}" ]]; then
        export HOME="/root"
    fi

    exec "${INSTALL_DIR}/bin/debopty" "$@"
}

if [[ "${DEBOPTY_INSTALL_LIB_ONLY:-0}" != "1" ]] && { [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]] || [[ -z "${BASH_SOURCE[0]:-}" ]]; }; then
    main "$@"
fi
