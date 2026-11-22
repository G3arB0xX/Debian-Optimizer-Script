#!/bin/bash
# =========================================================
# 通用工具模块 (标准 UI 与日志规范)
# =========================================================

# ----------------- 基础环境定义 -----------------
# Literal fallback + older-client probe target (kept in sync with VERSION).
VERSION_ID="324c3951"
_DEBOPTI_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=version.sh
source "${_DEBOPTI_ROOT}/scripts/version.sh"
debopti_load_version "${_DEBOPTI_ROOT}"

REMOTE_REPO="${DEBOPTY_REPO:-G3arB0xX/Debian-Optimizer-Script}"
REMOTE_VERSION_URL="https://raw.githubusercontent.com/${REMOTE_REPO}/main/VERSION"
REMOTE_COMMON_URL="https://raw.githubusercontent.com/${REMOTE_REPO}/main/scripts/common.sh"
REMOTE_ARCHIVE_URL="https://github.com/${REMOTE_REPO}/archive/refs/heads/main.tar.gz"

GREEN=$'\e[0;32m'
YELLOW=$'\e[1;33m'
RED=$'\e[0;31m'
CYAN=$'\e[0;36m'
DIM=$'\e[2m'
BOLD=$'\e[1m'
NC=$'\e[0m'

# 日志输出函数
info() { printf "${GREEN}🔹 %s${NC}\n" "$1" >&2; }
success() { printf "${GREEN}✨ %s${NC}\n" "$1" >&2; }
warn() { printf "${YELLOW}⚠️ %s${NC}\n" "$1" >&2; }
err()  { printf "${RED}‼️ %s${NC}\n" "$1" >&2; }
die()  { 
    printf "${RED}❌ %s${NC}\n" "$1" >&2
    if [[ "${IN_TUI:-}" == "true" ]]; then
        pause
        return 1
    else
        exit 1
    fi
}

# ----------------- 原子操作库 -----------------

# 安全安装软件包
safe_apt_install() {
    local pkgs=("$@")
    local missing_pkgs=()
    
    for pkg in "${pkgs[@]}"; do
        if ! dpkg -s "$pkg" >/dev/null 2>&1; then
            missing_pkgs+=("$pkg")
        fi
    done

    if [[ ${#missing_pkgs[@]} -gt 0 ]]; then
        info "正在补齐系统依赖: ${missing_pkgs[*]} ..."
        apt-get update -yq >/dev/null 2>&1
        apt-get install -yq "${missing_pkgs[@]}" || return 1
    fi
    return 0
}

# 创建系统用户
create_system_user() {
    local username=$1
    if ! id -u "$username" >/dev/null 2>&1; then
        info "创建系统用户: $username ..."
        useradd -r -s /usr/sbin/nologin "$username"
    fi
}

# 部署 Systemd 服务
deploy_systemd_service() {
    local svc_name=$1
    local svc_file="/etc/systemd/system/${svc_name}.service"
    
    info "部署 Systemd 服务: $svc_name ..."
    cat > "$svc_file"
    
    systemctl daemon-reload
    systemctl enable "$svc_name" >/dev/null 2>&1
    systemctl restart "$svc_name"
}

# 4. 注入 Systemd 服务安全补丁 (Override)
# 参数: 服务名, 补丁内容 (从标准输入读取)
inject_service_override() {
    local svc_name=$1
    local override_dir="/etc/systemd/system/${svc_name}.service.d"
    
    info "注入 Systemd 安全补丁: $svc_name ..."
    mkdir -p "$override_dir"
    cat > "${override_dir}/security.conf"
    
    systemctl daemon-reload
    # 如果服务正在运行，尝试重启以应用补丁
    systemctl is-active --quiet "$svc_name" && systemctl restart "$svc_name"
}

# 5. 幂等配置文件修改工具
# 参数: 文件路径, 键, 值, 分隔符(可选, 默认 '=')
set_conf_value() {
    local file=$1
    local key=$2
    local value=$3
    local sep=${4:-=}
    
    [[ ! -f "$file" ]] && touch "$file"
    
    if grep -q "^#\?${key}${sep}" "$file"; then
        # 存在则更新 (包括处理被注释的情况)
        sed -i "s|^#\?${key}${sep}.*|${key}${sep}${value}|" "$file"
    else
        # 不存在则追加
        echo "${key}${sep}${value}" >> "$file"
    fi
}

# ----------------- 模板渲染引擎 -----------------
# 参数: $1=模板物理路径, $2=输出目标路径, $3...=可选的额外的占位符替换对 (格式: "KEY=VALUE")
render_template() {
    local template="$1"
    local target="$2"
    shift 2
    
    # 确定 SCRIPT_DIR
    local script_dir="${SCRIPT_DIR:-/opt/debopty}"
    
    # 如果 template 不是绝对路径，尝试拼接 SCRIPT_DIR
    if [[ "$template" != /* ]]; then
        template="${script_dir}/${template}"
    fi
    
    if [[ ! -f "$template" ]]; then
        err "模板文件未找到: $template"
        return 1
    fi
    
    # 1. 读取模板内容
    local content
    content=$(cat "$template")
    
    # 2. 获取常用全局变量并替换
    local sot_user
    sot_user=$(get_sot_user || echo "root")
    local initial_user
    initial_user=$(get_initial_user || echo "root")
    local bin_link="${BIN_LINK:-/usr/local/bin/debopty}"
    local nft_base_d="/etc/nftables.d"
    
    content="${content//\{\{DEBOPTI_VERSION\}\}/${DEBOPTI_VERSION:-}}"
    content="${content//\{\{DEBOPTI_BUILD\}\}/${DEBOPTI_BUILD:-${VERSION_ID:-}}}"
    content="${content//\{\{VERSION_ID\}\}/${VERSION_ID:-}}"
    content="${content//\{\{IS_CN_REGION\}\}/${IS_CN_REGION:-false}}"
    content="${content//\{\{SOT_USER\}\}/$sot_user}"
    content="${content//\{\{INITIAL_USER\}\}/$initial_user}"
    content="${content//\{\{INSTALL_DIR\}\}/${INSTALL_DIR:-/opt/debopty}}"
    content="${content//\{\{BIN_LINK\}\}/$bin_link}"
    content="${content//\{\{NFT_BASE_D\}\}/$nft_base_d}"
    
    # 3. 动态替换传入的 KV 对
    for arg in "$@"; do
        local key="${arg%%=*}"
        local val="${arg#*=}"
        content="${content//\{\{${key}\}\}/$val}"
    done
    
    # 4. 写入目标
    if [[ "$target" == "-" ]]; then
        echo "$content"
    else
        mkdir -p "$(dirname "$target")"
        echo "$content" > "$target"
    fi
}

# ----------------- 持久化配置管理 -----------------
CONFIG_FILE="/etc/debopti/debopti.conf"

# 加载全局配置
load_project_config() {
    if [[ ! -f "$CONFIG_FILE" ]]; then
        render_template "templates/common/debopti.conf" "$CONFIG_FILE"
    fi
    # shellcheck disable=SC1090
    source "$CONFIG_FILE"
}

# 保存配置项 (带注释保护)
# 参数: 键, 值
save_project_config() {
    local key=$1
    local value=$2
    set_conf_value "$CONFIG_FILE" "$key" "$value"
}

# ----------------- 交互逻辑 -----------------

# 暂停函数：等待用户确认
pause() {
    echo -e "\n${YELLOW}⌨️  执行完毕。按任意键继续...${NC}"
    read -n 1 -s -r -p ""
}

# ----------------- 系统与环境状态 -----------------

# 补全运行时 HOME（容器或非登录 shell 可能未设置）
ensure_runtime_home() {
    [[ -n "${HOME:-}" ]] && return 0
    if [[ $EUID -eq 0 ]]; then
        export HOME="/root"
    elif command -v getent >/dev/null 2>&1; then
        HOME="$(getent passwd "$(id -un)" | cut -d: -f6)"
        export HOME="${HOME:-/tmp}"
    else
        export HOME="/tmp"
    fi
}

# 获取系统中的第一个普通用户 (UID >= 1000, 排除 nobody)
get_normal_user() {
    local user
    user=$(awk -F: '$3 >= 1000 && $1 != "nobody" {print $1}' /etc/passwd | head -n 1)
    echo "$user"
}

# 获取 SOT (真理源) 用户
get_sot_user() {
    local normal_user
    normal_user=$(get_normal_user)
    if [[ -n "$normal_user" ]]; then
        echo "$normal_user"
    else
        echo "root"
    fi
}

# 获取系统中的所有真实用户 (UID >= 1000, 排除 nobody, 且拥有合法 Shell 与 Home 目录) + root
get_all_real_users() {
    local users=()
    users+=("root")
    while IFS=: read -r -a fields; do
        [[ ${#fields[@]} -lt 7 ]] && continue
        local username="${fields[0]}"
        local uid="${fields[2]}"
        local homedir="${fields[5]}"
        local shell="${fields[6]}"
        
        if [[ "$uid" -ge 1000 && "$username" != "nobody" ]]; then
            if [[ -d "$homedir" ]] && [[ "$shell" != *"/nologin" ]] && [[ "$shell" != *"/false" ]]; then
                users+=("$username")
            fi
        fi
    done < /etc/passwd
    echo "${users[@]}"
}

# 获取最初运行 debopty 的非 root 用户 (通过 sudo 运行则为 SUDO_USER，否则为 root)
get_initial_user() {
    if [[ -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
        echo "$SUDO_USER"
    else
        echo "root"
    fi
}

# 动态配置系统全局环境变量 (/etc/environment)
# 参数: $1=变量名, $2=变量值
set_system_env() {
    local key=$1
    local val=$2
    local env_file="/etc/environment"
    
    if [[ ! -f "$env_file" ]]; then
        touch "$env_file"
    fi
    
    # 幂等性写入: 如果变量已存在则更新，不存在则追加
    if grep -q "^${key}=" "$env_file"; then
        sed -i "s|^${key}=.*|${key}=\"${val}\"|" "$env_file"
    else
        echo "${key}=\"${val}\"" >> "$env_file"
    fi
}

# 移除系统全局环境变量 (/etc/environment)
# 参数: $1=变量名
remove_system_env() {
    local key=$1
    local env_file="/etc/environment"
    if [[ -f "$env_file" ]]; then
        sed -i "/^${key}=/d" "$env_file"
    fi
}

# 动态配置 Fish 环境变量 (物理应用至真理源 SOT 用户)
# 参数: $1=变量名, $2=变量值
update_fish_env() {
    local var_name=$1
    local var_value=$2
    
    # 检查 fish 是否安装
    if ! command -v fish >/dev/null 2>&1; then
        return 0
    fi

    local sot_user
    sot_user=$(get_sot_user)
    info "正在同步 SOT ($sot_user) Fish 环境变量: $var_name ..."

    local user_home
    user_home=$(eval echo "~$sot_user")
    [[ ! -d "$user_home" ]] && return 0
    
    local conf_d="$user_home/.config/fish/conf.d"
    local env_file="$conf_d/debopti_vars.fish"

    if [[ ! -d "$conf_d" ]]; then
        mkdir -p "$conf_d"
        chown -R "$sot_user:$sot_user" "$user_home/.config" 2>/dev/null || true
    fi
    
    # 幂等性写入: 如果变量已存在则更新，不存在则追加
    if grep -q "set -gx $var_name " "$env_file" 2>/dev/null; then
        sed -i "s|set -gx $var_name .*|set -gx $var_name $var_value|" "$env_file"
    else
        echo "set -gx $var_name $var_value" >> "$env_file"
    fi
    
    # 确保权限正确，且其他普通用户可读
    chown "$sot_user:$sot_user" "$env_file" 2>/dev/null || true
    chmod o+r "$env_file" 2>/dev/null || true
}

# 动态配置 Fish PATH (物理应用至真理源 SOT 用户)
# 参数: $1=路径
update_fish_path() {
    local target_path=$1
    
    # 检查 fish 是否安装
    if ! command -v fish >/dev/null 2>&1; then
        return 0
    fi

    local sot_user
    sot_user=$(get_sot_user)
    info "正在同步 SOT ($sot_user) Fish PATH: $target_path ..."

    local user_home
    user_home=$(eval echo "~$sot_user")
    [[ ! -d "$user_home" ]] && return 0

    local conf_d="$user_home/.config/fish/conf.d"
    local path_file="$conf_d/debopti_path.fish"

    if [[ ! -d "$conf_d" ]]; then
        mkdir -p "$conf_d"
        chown -R "$sot_user:$sot_user" "$user_home/.config" 2>/dev/null || true
    fi
    
    # 幂等性写入: 使用 fish_add_path (fish 3.2+)
    local fish_version=$(fish --version 2>/dev/null | awk '{print $3}' || echo "0.0")
    if [[ $(echo "$fish_version 3.2" | awk '{print ($1 >= $2)}') -eq 1 ]]; then
        if ! grep -q "fish_add_path $target_path" "$path_file" 2>/dev/null; then
            echo "fish_add_path $target_path" >> "$path_file"
        fi
    else
        if ! grep -q "contains $target_path \$PATH" "$path_file" 2>/dev/null; then
            echo "if not contains $target_path \$PATH; set -gx PATH \$PATH $target_path; end" >> "$path_file"
        fi
    fi
    
    chown "$sot_user:$sot_user" "$path_file" 2>/dev/null || true
    chmod o+r "$path_file" 2>/dev/null || true
}

# 移除 Fish 环境变量
remove_fish_env() {
    local var_name=$1
    if ! command -v fish >/dev/null 2>&1; then return 0; fi

    local sot_user
    sot_user=$(get_sot_user)
    local user_home
    user_home=$(eval echo "~$sot_user")
    local env_file="$user_home/.config/fish/conf.d/debopti_vars.fish"
    if [[ -f "$env_file" ]]; then
        sed -i "/set -gx $var_name /d" "$env_file"
    fi
    return 0
}

# 移除 Fish PATH
remove_fish_path() {
    local target_path=$1
    if ! command -v fish >/dev/null 2>&1; then return 0; fi

    local sot_user
    sot_user=$(get_sot_user)
    local user_home
    user_home=$(eval echo "~$sot_user")
    local path_file="$user_home/.config/fish/conf.d/debopti_path.fish"
    if [[ -f "$path_file" ]]; then
        sed -i "s|.*$target_path.*||g" "$path_file"
    fi
    return 0
}

# ----------------- 脚本维护功能 -----------------

_debopti_curl_to() {
    local url=$1 dest=$2
    curl -fsSL --connect-timeout 10 --max-time 30 \
        -H 'Cache-Control: no-cache, no-store, must-revalidate' \
        -H 'Pragma: no-cache' \
        -o "$dest" "$url" 2>/dev/null && [[ -s "$dest" ]]
}

_debopti_identity_from_tmp() {
    local tmp=$1
    debopti_parse_version_file "$tmp" 2>/dev/null || debopti_parse_version_text "$(cat "$tmp")"
}

_debopti_persist_identity() {
    local version build
    version="$(debopti_normalize_calver "$1")"
    build=$2
    if declare -f save_project_config >/dev/null 2>&1 && [[ -n "${CONFIG_FILE:-}" ]]; then
        mkdir -p "$(dirname "$CONFIG_FILE")"
        save_project_config "SCRIPT_VERSION" "$version"
        save_project_config "SCRIPT_BUILD" "$build"
    fi
}

# 已安装身份：VERSION 文件优先，回退 common.sh 中的 VERSION_ID
get_installed_identity() {
    local root="${INSTALL_DIR:-/opt/debopty}" parsed
    if [[ -f "${root}/VERSION" ]] && parsed="$(debopti_parse_version_file "${root}/VERSION")"; then
        printf '%s\n' "$parsed"
        return 0
    fi
    if [[ -f "${root}/scripts/common.sh" ]] && parsed="$(debopti_parse_version_text "$(cat "${root}/scripts/common.sh")")"; then
        printf '%s\n' "$parsed"
        return 0
    fi
    printf '%s|%s\n' "${DEBOPTI_VERSION:-}" "${DEBOPTI_BUILD:-${VERSION_ID:-}}"
}

get_installed_version_id() {
    local id
    id="$(get_installed_identity)"
    printf '%s\n' "${id#*|}"
}

_debopti_try_identity_target() {
    local tmp=$1 raw_base=$2 repo_rel=$3 cache_q=$4
    local api_path="https://api.github.com/repos/${REMOTE_REPO}/contents/${repo_rel}?ref=main&${cache_q}"
    local parsed proxy

    if [[ "${IS_CN_REGION:-false}" == "true" ]]; then
        local cn_raw_proxies=(
            "https://gh-proxy.com"
            "https://cdn.gh-proxy.org"
            "https://edgeone.gh-proxy.org"
            "https://git.yylx.win"
            "https://gh-proxy.org"
            "https://ghfast.top"
            "https://wget.la"
        )
        for proxy in "${cn_raw_proxies[@]}"; do
            if _debopti_curl_to "${proxy}/${raw_base}?${cache_q}" "$tmp" \
                && parsed="$(_debopti_identity_from_tmp "$tmp")"; then
                printf '%s\n' "$parsed"
                return 0
            fi
        done

        local cn_api_proxies=(
            "https://gh-proxy.com"
            "https://edgeone.gh-proxy.org"
            "https://gh-proxy.org"
        )
        for proxy in "${cn_api_proxies[@]}"; do
            if curl -fsSL --connect-timeout 10 --max-time 30 \
                -H 'Accept: application/vnd.github.raw' \
                -H 'Cache-Control: no-cache' \
                -o "$tmp" "${proxy}/${api_path}" 2>/dev/null && [[ -s "$tmp" ]] \
                && parsed="$(_debopti_identity_from_tmp "$tmp")"; then
                printf '%s\n' "$parsed"
                return 0
            fi
        done
    fi

    if _debopti_curl_to "${raw_base}?${cache_q}" "$tmp" \
        && parsed="$(_debopti_identity_from_tmp "$tmp")"; then
        printf '%s\n' "$parsed"
        return 0
    fi

    if curl -fsSL --connect-timeout 10 --max-time 30 \
        -H 'Accept: application/vnd.github.raw' \
        -H 'Cache-Control: no-cache' \
        -o "$tmp" "$api_path" 2>/dev/null && [[ -s "$tmp" ]] \
        && parsed="$(_debopti_identity_from_tmp "$tmp")"; then
        printf '%s\n' "$parsed"
        return 0
    fi

    if declare -f download_with_fallback >/dev/null 2>&1 \
        && download_with_fallback "$tmp" "${raw_base}?${cache_q}" \
        && parsed="$(_debopti_identity_from_tmp "$tmp")"; then
        printf '%s\n' "$parsed"
        return 0
    fi

    return 1
}

# 拉取远程身份（VERSION 优先，失败再读 common.sh）。打印 version|build
fetch_remote_identity() {
    local tmp cache_q parsed
    local version_base common_base

    tmp="$(mktemp /tmp/debopti_remote_identity.XXXXXX)"
    cache_q="debopti=$(date +%s)-${RANDOM}"
    version_base="${REMOTE_VERSION_URL%%\?*}"
    common_base="${REMOTE_COMMON_URL%%\?*}"

    if parsed="$(_debopti_try_identity_target "$tmp" "$version_base" "VERSION" "$cache_q")"; then
        rm -f "$tmp"
        printf '%s\n' "$parsed"
        return 0
    fi

    if parsed="$(_debopti_try_identity_target "$tmp" "$common_base" "scripts/common.sh" "$cache_q")"; then
        rm -f "$tmp"
        printf '%s\n' "$parsed"
        return 0
    fi

    rm -f "$tmp"
    return 1
}

fetch_remote_version_id() {
    local id
    id="$(fetch_remote_identity)" || return 1
    printf '%s\n' "${id#*|}"
}

_debopti_apply_tree() {
    local src=$1 dest=$2
    rm -rf "${src}/.git" "${src}/.jj" "${src}/.github"
    mkdir -p "$dest"
    if ! command -v rsync >/dev/null 2>&1; then
        safe_apt_install rsync || true
    fi
    if command -v rsync >/dev/null 2>&1; then
        rsync -a --delete \
            --exclude '.git/' --exclude '.jj/' --exclude '.github/' \
            "${src}/" "${dest}/"
    else
        cp -a "${src}/." "${dest}/"
    fi
}

_debopti_extract_archive() {
    local archive_url=$1 dest=$2
    local tmp_dir tmp_tar
    tmp_dir="$(mktemp -d /tmp/debopti_update.XXXXXX)"
    tmp_tar="${tmp_dir}/src.tar.gz"
    if ! download_with_fallback "$tmp_tar" "${archive_url}?$(date +%s)-${RANDOM}"; then
        rm -rf "$tmp_dir"
        return 1
    fi
    mkdir -p "$dest"
    if ! tar -xzf "$tmp_tar" -C "$dest" --strip-components=1; then
        rm -rf "$tmp_dir" "$dest"
        return 1
    fi
    rm -rf "$tmp_dir"
    return 0
}

# 1. 脚本在线更新 (Build 比对 + tag 包优先 + 解压校验)
script_update() {
    info "正在检查脚本更新..."

    local installed_id remote_id
    local inst_ver inst_build remote_ver remote_build
    local force_sync="false"

    installed_id="$(get_installed_identity)"
    inst_ver="${installed_id%%|*}"
    inst_build="${installed_id#*|}"
    [[ -n "$inst_build" ]] || inst_build="${VERSION_ID:-}"

    if ! remote_id="$(fetch_remote_identity)"; then
        err "无法获取远程版本信息，请检查网络连接。"
        return 1
    fi
    remote_ver="$(debopti_normalize_calver "${remote_id%%|*}")"
    remote_build="${remote_id#*|}"

    if ! debopti_needs_update "$inst_build" "$remote_build"; then
        success "远程版本与本地一致 ($(debopti_format_prompt "$inst_ver" "$inst_build"))。"
        read -p "是否仍强制从 GitHub 同步最新代码？[y/N]: " force_confirm
        [[ ! "$force_confirm" =~ ^[Yy]$ ]] && return 0
        force_sync="true"
    else
        warn "检测到新版本: $(debopti_format_prompt "$remote_ver" "$remote_build") (当前: $(debopti_format_prompt "$inst_ver" "$inst_build"))"
        read -p "是否立即更新？[y/N]: " confirm
        [[ ! "$confirm" =~ ^[Yy]$ ]] && return 0
    fi

    info "正在执行原子更新..."

    local install_root="${INSTALL_DIR:-/opt/debopty}"
    if [[ "$install_root" == "/opt/debopty" && ! -d "$install_root" && -d "/opt/debopti" ]]; then
        mv "/opt/debopti" "$install_root"
    fi
    local extract_dir urls url extracted downloaded

    if [[ -d "${install_root}/.git" ]]; then
        (
            cd "$install_root" || exit 1
            git fetch origin main --force --prune
            git reset --hard origin/main
        ) || {
            err "Git 同步失败。"
            return 1
        }
        if [[ "$force_sync" != "true" ]] && ! debopti_verify_tree "$install_root" "$remote_ver" "$remote_build"; then
            err "更新后的版本元数据与预期不符，已中止。"
            return 1
        fi
    else
        extract_dir="$(mktemp -d /tmp/debopti_extract.XXXXXX)"
        urls=()
        extracted="false"
        downloaded="false"
        if [[ "$force_sync" != "true" && -n "$remote_ver" ]]; then
            urls+=("https://github.com/${REMOTE_REPO}/archive/refs/tags/$(debopti_tag_name "$remote_ver" "$remote_build").tar.gz")
        fi
        urls+=("$REMOTE_ARCHIVE_URL")
        urls+=("https://codeload.github.com/${REMOTE_REPO}/tar.gz/refs/heads/main")

        for url in "${urls[@]}"; do
            rm -rf "$extract_dir"
            mkdir -p "$extract_dir"
            if ! _debopti_extract_archive "$url" "$extract_dir"; then
                continue
            fi
            downloaded="true"
            if [[ "$force_sync" == "true" ]] || debopti_verify_tree "$extract_dir" "$remote_ver" "$remote_build"; then
                extracted="true"
                break
            fi
            warn "压缩包与预期 Build 不一致，尝试下一来源..."
        done

        if [[ "$extracted" != "true" ]]; then
            rm -rf "$extract_dir"
            if [[ "$downloaded" != "true" ]]; then
                err "远程同步失败。"
            else
                err "压缩包版本元数据与预期不符，已中止。"
            fi
            return 1
        fi

        _debopti_apply_tree "$extract_dir" "$install_root"
        rm -rf "$extract_dir"
    fi

    chmod +x "${install_root}/deb_optimizer.sh"
    local bin_link="${BIN_LINK:-/usr/local/bin/debopty}"
    ln -sf "${install_root}/deb_optimizer.sh" "$bin_link"
    if [[ -L "/usr/local/bin/debopti" ]]; then
        local legacy_target=""
        legacy_target="$(readlink -f "/usr/local/bin/debopti" 2>/dev/null || true)"
        if [[ "$legacy_target" == *"deb_optimizer.sh" || ! -e "/usr/local/bin/debopti" ]]; then
            rm -f "/usr/local/bin/debopti"
        fi
    fi
    rm -f "/etc/profile.d/debopti.sh" "/etc/fish/conf.d/debopti.fish"
    if [[ -f "${install_root}/templates/common/debopty.sh" ]]; then
        render_template "${install_root}/templates/common/debopty.sh" "/etc/profile.d/debopty.sh" "BIN_LINK=$bin_link"
        chmod +x /etc/profile.d/debopty.sh
    fi
    if command -v fish >/dev/null 2>&1 && [[ -f "${install_root}/templates/common/debopty.fish" ]]; then
        render_template "${install_root}/templates/common/debopty.fish" "/etc/fish/conf.d/debopty.fish" "BIN_LINK=$bin_link"
    fi
    _debopti_persist_identity "$remote_ver" "$remote_build"
    success "更新成功！正在重新载入脚本..."
    sleep 1
    exec "${install_root}/deb_optimizer.sh"
}

# 2. 脚本完全卸载
script_uninstall() {
    warn "警告：此操作将移除所有已安装的优化逻辑、脚本资产及全局命令。"
    read -p "确定要彻底卸载 debopty 吗？[y/N]: " confirm
    [[ ! "$confirm" =~ ^[Yy]$ ]] && return

    info "正在卸载脚本..."

    # 1. 移除全局命令与 Shell 环境注入
    rm -f "/usr/local/bin/debopty" "/usr/local/bin/debopti" \
        "/etc/profile.d/debopty.sh" "/etc/profile.d/debopti.sh" \
        "/etc/fish/conf.d/debopty.fish" "/etc/fish/conf.d/debopti.fish"

    # 2. 移除 Fish 环境增强
    remove_fish_path "/opt/debopty"
    remove_fish_path "/opt/debopti"
    remove_fish_path "/usr/local/go/bin"
    remove_fish_path "$HOME/go/bin"
    remove_fish_env "IS_CN_REGION"

    # 3. 询问是否保留配置
    read -p "是否保留配置文件 (/etc/debopti)？[Y/n]: " keep_conf
    if [[ "$keep_conf" =~ ^[Nn]$ ]]; then
        rm -rf "/etc/debopti"
        info "已清理配置文件。"
    fi

    # 4. 移除主安装目录
    rm -rf "/opt/debopty" "/opt/debopti"
    
    success "卸载完成。系统已恢复至脚本安装前的状态（不包括已修改的内核/防火墙配置）。"
    exit 0
}
