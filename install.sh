#!/bin/bash
# =========================================================
# debopty (Debian Optimizer Script) 一键安装自举脚本 (Bootstrap)
# =========================================================
set -euo pipefail

# 零 curl/wget 兼容下载自举函数 (curl -> wget -> python3 -> perl -> apt-get)
_debopty_bootstrap_fetch() {
    local url=$1
    local dest=$2
    local sudo_prefix=()
    if [[ ${EUID:-$(id -u)} -ne 0 && "${DEBOPTY_NO_SUDO:-0}" != "1" ]] && command -v sudo >/dev/null 2>&1; then
        sudo_prefix=(sudo)
    fi

    rm -f "$dest"
    if command -v curl >/dev/null 2>&1; then
        if curl -fsSL --connect-timeout 10 --max-time 60 "$url" -o "$dest" 2>/dev/null && [[ -s "$dest" ]]; then
            return 0
        fi
        rm -f "$dest"
    fi

    if command -v wget >/dev/null 2>&1; then
        if wget -qO "$dest" --timeout=15 "$url" 2>/dev/null && [[ -s "$dest" ]]; then
            return 0
        fi
        rm -f "$dest"
    fi

    if command -v python3 >/dev/null 2>&1; then
        if python3 -c 'import sys,urllib.request; urllib.request.urlretrieve(sys.argv[1], sys.argv[2])' "$url" "$dest" 2>/dev/null && [[ -s "$dest" ]]; then
            return 0
        fi
        rm -f "$dest"
    fi

    if command -v perl >/dev/null 2>&1; then
        if perl -MHTTP::Tiny -e 'my $r=HTTP::Tiny->new(timeout=>15)->get($ARGV[0]); exit 1 unless $r->{success}; open my $fh,">",$ARGV[1] or exit 1; binmode $fh; print $fh $r->{content}' "$url" "$dest" 2>/dev/null && [[ -s "$dest" ]]; then
            return 0
        fi
        rm -f "$dest"
    fi

    if command -v apt-get >/dev/null 2>&1; then
        "${sudo_prefix[@]}" apt-get update -yq >/dev/null 2>&1 || true
        if ! "${sudo_prefix[@]}" apt-get install -yq ca-certificates curl >/dev/null 2>&1; then
            "${sudo_prefix[@]}" apt-get install -yq --fix-missing ca-certificates curl >/dev/null 2>&1 || true
        fi
        hash -r
        if command -v curl >/dev/null 2>&1; then
            if curl -fsSL --connect-timeout 10 --max-time 60 "$url" -o "$dest" 2>/dev/null && [[ -s "$dest" ]]; then
                return 0
            fi
        fi
    fi

    rm -f "$dest"
    return 1
}

fetch_url() {
    local url=$1
    if command -v curl >/dev/null 2>&1; then
        curl -sL --connect-timeout 3 "$url" 2>/dev/null && return 0
    fi
    if command -v wget >/dev/null 2>&1; then
        wget -qO- --timeout=3 "$url" 2>/dev/null && return 0
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import sys,urllib.request; print(urllib.request.urlopen(sys.argv[1], timeout=3).read().decode("utf-8", "ignore"))' "$url" 2>/dev/null && return 0
    fi
    if command -v perl >/dev/null 2>&1; then
        perl -MHTTP::Tiny -e 'my $r=HTTP::Tiny->new(timeout=>3)->get($ARGV[0]); exit 1 unless $r->{success}; print $r->{content}' "$url" 2>/dev/null && return 0
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

_debopty_verify_sha256() {
    local file=$1
    local sums_file=$2
    local asset_name=$3
    [[ -f "$file" && -s "$file" && -f "$sums_file" && -s "$sums_file" && -n "$asset_name" ]] || return 1

    local expected="" line hash fname
    while IFS= read -r line; do
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

    [[ -n "$expected" ]] || return 1

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
        return 1
    fi

    actual="${actual,,}"
    [[ -n "$actual" && "$actual" == "$expected" ]]
}

_debopty_fetch_release_asset() {
    local is_cn=$1
    local url=$2
    local dest=$3

    if [[ "$is_cn" == "true" ]] && [[ "$url" =~ github\.com|githubusercontent\.com ]]; then
        local mirrors=(
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

            if _debopty_bootstrap_fetch "$dl_url" "$dest"; then
                return 0
            fi
        done
        return 1
    fi

    _debopty_bootstrap_fetch "$url" "$dest"
}

_debopty_install_rust_binary() {
    local install_dir=$1
    local is_cn=${2:-false}
    local tag=${3:-${DEBOPTY_RELEASE_TAG:-}}
    local mode="${DEBOPTY_INSTALL_BINARY:-auto}"

    if [[ "$mode" == "0" || "$mode" == "false" ]]; then
        return 0
    fi

    local arch
    if ! arch="$(_debopty_detect_arch)"; then
        return 1
    fi

    local repo="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"
    local tmp_dir
    if ! tmp_dir="$(mktemp -d "${TMPDIR:-/tmp}/debopty-bin.XXXXXX")"; then
        return 1
    fi

    if [[ -z "$tag" && -z "${DEBOPTY_RELEASE_BASE_URL:-}" ]]; then
        local latest_json="${tmp_dir}/latest.json"
        if _debopty_fetch_release_asset "$is_cn" "https://api.github.com/repos/${repo}/releases/latest" "$latest_json"; then
            local line
            while IFS= read -r line; do
                if [[ "$line" =~ \"tag_name\"[[:space:]]*:[[:space:]]*\"([^\"]+)\" ]]; then
                    tag="${BASH_REMATCH[1]}"
                    break
                fi
            done < "$latest_json"
        fi
    fi

    if [[ -z "$tag" && -f "${install_dir}/VERSION" ]]; then
        local vline
        while IFS= read -r vline; do
            if [[ "$vline" =~ ^DEBOPTI_SEMVER=[\"\']?([^\"\']+)[\"\']? ]]; then
                tag="v${BASH_REMATCH[1]}"
                break
            fi
        done < "${install_dir}/VERSION"
    fi

    if [[ -z "$tag" ]]; then
        rm -rf "$tmp_dir"
        return 1
    fi

    if [[ "$tag" =~ ^[0-9] ]]; then
        tag="v${tag}"
    fi

    local asset_name="debopty-${tag}-linux-${arch}.tar.gz"
    local base_url
    if [[ -n "${DEBOPTY_RELEASE_BASE_URL:-}" ]]; then
        base_url="${DEBOPTY_RELEASE_BASE_URL%/}"
    else
        base_url="https://github.com/${repo}/releases/download/${tag}"
    fi

    local sums_file="${tmp_dir}/SHA256SUMS"
    local archive_file="${tmp_dir}/${asset_name}"
    if ! _debopty_fetch_release_asset "$is_cn" "${base_url}/SHA256SUMS" "$sums_file"; then
        rm -rf "$tmp_dir"
        return 1
    fi
    if ! _debopty_fetch_release_asset "$is_cn" "${base_url}/${asset_name}" "$archive_file"; then
        rm -rf "$tmp_dir"
        return 1
    fi
    if ! _debopty_verify_sha256 "$archive_file" "$sums_file" "$asset_name"; then
        rm -rf "$tmp_dir"
        return 1
    fi

    mkdir -p "${tmp_dir}/extract"
    if ! tar -xzf "$archive_file" -C "${tmp_dir}/extract" 2>/dev/null; then
        rm -rf "$tmp_dir"
        return 1
    fi
    if [[ ! -f "${tmp_dir}/extract/debopty" ]]; then
        rm -rf "$tmp_dir"
        return 1
    fi

    mkdir -p "${install_dir}/bin"
    cp "${tmp_dir}/extract/debopty" "${install_dir}/bin/debopty"
    chmod 0755 "${install_dir}/bin/debopty"

    local rs_link="${DEBOPTY_RS_BIN_LINK:-/usr/local/bin/debopty-rs}"
    mkdir -p "$(dirname "$rs_link")" 2>/dev/null || true
    ln -sf "${install_dir}/bin/debopty" "$rs_link" 2>/dev/null || true

    rm -rf "$tmp_dir"
    return 0
}

main() {
    local install_url="${DEBOPTY_INSTALL_URL:-https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh}"

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
            echo -e "     \033[1;36msu - -c 'u=\"https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh\"; f=\$(mktemp); trap \"rm -f \\\"\$f\\\"\" EXIT; curl -fsSL \"\$u\" -o \"\$f\" 2>/dev/null || wget -qO \"\$f\" \"\$u\" 2>/dev/null || python3 -c \"import sys,urllib.request;urllib.request.urlretrieve(sys.argv[1],sys.argv[2])\" \"\$u\" \"\$f\" 2>/dev/null || { apt-get update -yq && apt-get install -yq ca-certificates curl && curl -fsSL \"\$u\" -o \"\$f\"; }; bash \"\$f\"'\033[0m"
            echo -e "  2. 安装 sudo 并将当前用户加入 sudoers 组后再试。"
            exit 1
        fi
    fi

    # 2. 基础环境自举 (补齐 ca-certificates/curl/git/wget)
    echo -e "\033[0;32m[1/4] 正在检查基础环境...\033[0m"

    # 临时 DNS 修复逻辑：若当前 DNS 无法解析 github 域名，写入临时 DNS (1.1.1.1/8.8.8.8) 恢复连接
    if ! getent hosts raw.githubusercontent.com >/dev/null 2>&1 && ! getent hosts github.com >/dev/null 2>&1; then
        echo -e "\033[1;33m⚠️  检测到 DNS 解析故障，正在写入临时公共 DNS (1.1.1.1/8.8.8.8) 恢复连接...\033[0m"
        if [[ -f /etc/resolv.conf && ! -f /etc/resolv.conf.debopti.bak ]]; then
            cp /etc/resolv.conf /etc/resolv.conf.debopti.bak
        fi
        cat > /etc/resolv.conf << 'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
EOF
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

    if [[ "$(fetch_url https://ipinfo.io/country)" == "CN" ]]; then
        IS_CN=true
    elif fetch_url http://myip.ipip.net | grep -q "中国"; then
        IS_CN=true
    fi

    # 4. 仓库克隆与同步
    INSTALL_DIR="/opt/debopty"
    REPO_URL="https://github.com/G3arB0xX/Debian-Optimizer-Script.git"
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

    echo -e "\033[0;32m[3/4] 正在拉取项目资产至 $INSTALL_DIR ...\033[0m"
    local cloned="false"
    local candidate_url
    for candidate_url in "${clone_candidates[@]}"; do
        rm -rf "$INSTALL_DIR"
        if git clone "$candidate_url" "$INSTALL_DIR"; then
            cloned="true"
            break
        fi
        echo -e "\033[1;33m[提示] 节点拉取失败，尝试下一加速镜像...\033[0m"
    done
    if [[ "$cloned" != "true" ]]; then
        echo -e "\033[0;31m[错误] 仓库拉取失败，请检查网络连接。\033[0m"
        exit 1
    fi

    # 5. 移交控制权
    echo -e "\033[0;32m[4/4] 正在启动主程序...\033[0m"
    chmod +x "$INSTALL_DIR/deb_optimizer.sh"
    cd "$INSTALL_DIR"

    # 清理开发元数据，保持生产环境纯净
    rm -rf .git .jj .github .jj_backup .cursor tools governance instructions tests rust AGENTS.md CLAUDE.md GEMINI.md Makefile .shellcheckrc .editorconfig .gitignore
    find "$INSTALL_DIR" -type f -name 'INDEX.md' -delete 2>/dev/null || true

    # 二进制优先安装：下载并校验静态 Rust 二进制（失败时回退纯 Bash 引擎）
    _debopty_install_rust_binary "$INSTALL_DIR" "$IS_CN" || echo -e "\033[1;33m警告: 未能获取预编译二进制，将继续使用纯 Bash 引擎。\033[0m"

    # 容器或非登录 shell 可能未设置 HOME，启动主程序前补全
    # shellcheck disable=SC1091
    source "$INSTALL_DIR/scripts/common.sh"
    ensure_runtime_home

    exec bash "$INSTALL_DIR/deb_optimizer.sh"
}

if [[ "${DEBOPTY_INSTALL_LIB_ONLY:-0}" != "1" ]] && { [[ "${BASH_SOURCE[0]:-}" == "${0:-}" ]] || [[ -z "${BASH_SOURCE[0]:-}" ]]; }; then
    main "$@"
fi
