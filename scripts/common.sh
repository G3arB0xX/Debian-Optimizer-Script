#!/bin/bash
# =========================================================
# 通用工具模块 (标准 UI 与日志规范)
# =========================================================

# ----------------- 基础环境定义 -----------------
VERSION_ID="wsukmmvs"

# 云端版本描述 (用于对比)
REMOTE_VERSION_URL="https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/scripts/common.sh"

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
        return 1 2>/dev/null || exit 1
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

# ----------------- 持久化配置管理 -----------------
CONFIG_FILE="/etc/debopti/debopti.conf"

# 加载全局配置
load_project_config() {
    if [[ ! -f "$CONFIG_FILE" ]]; then
        mkdir -p "$(dirname "$CONFIG_FILE")"
        cat > "$CONFIG_FILE" <<EOF
# =========================================================
# Debian Optimizer Script 全局持久化配置文件
# =========================================================

# 是否位于中国大陆 (true/false)
# 用于自动切换 APT 镜像、GitHub 加速以及环境变量代理
IS_CN_REGION=""

# 基础优化完成标记 (true/false)
# 标记系统是否已完成内核调优、安全加固等基础流程
BASE_OPTIMIZED="false"

# 脚本安装标记 (true/false)
# 标记脚本是否已完成自举安装并绑定全局命令
INSTALLED="false"

# SSH 安全加固标记 (true/false)
# 标记是否已完成密钥登录强制化与端口随机化
SSH_HARDENED="false"

# 默认使用的文本编辑器
EDITOR_CMD="micro"
EOF
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

# 获取最初运行 debopti 的非 root 用户 (通过 sudo 运行则为 SUDO_USER，否则为 root)
get_initial_user() {
    if [[ -n "${SUDO_USER:-}" && "$SUDO_USER" != "root" ]]; then
        echo "$SUDO_USER"
    else
        echo "root"
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

# 1. 脚本在线更新 (基于版本比对)
script_update() {
    info "正在检查脚本更新..."
    
    # 尝试获取远程版本
    local remote_version
    remote_version=$(curl -sL "$REMOTE_VERSION_URL" | grep "VERSION_ID=" | head -n 1 | cut -d'"' -f2)
    
    if [[ -z "$remote_version" ]]; then
        err "无法获取远程版本信息，请检查网络连接。"
        return 1
    fi
    
    if [[ "$remote_version" == "$VERSION_ID" ]]; then
        success "当前已是最新版本 ($VERSION_ID)。"
        return 0
    fi
    
    warn "检测到新版本: $remote_version (当前: $VERSION_ID)"
    read -p "是否立即更新？[y/N]: " confirm
    [[ ! "$confirm" =~ ^[Yy]$ ]] && return 0

    info "正在执行原子更新..."
    
    # 检查是否为 git 仓库
    if [[ -d "${INSTALL_DIR:-/opt/debopti}/.git" ]]; then
        cd "${INSTALL_DIR:-/opt/debopti}"
        git fetch --all && git reset --hard origin/main
    else
        # 非 Git 模式：拉取仓库压缩包并解压覆盖
        local tmp_dir="/tmp/debopti_update"
        local archive_url="https://github.com/G3arB0xX/Debian-Optimizer-Script/archive/refs/heads/main.tar.gz"
        local tmp_tar="/tmp/debopti_update.tar.gz"
        
        mkdir -p "$tmp_dir"
        if download_with_fallback "$tmp_tar" "$archive_url"; then
            tar -xzf "$tmp_tar" -C "$tmp_dir" --strip-components=1
            cp -r "${tmp_dir}/"* "${INSTALL_DIR:-/opt/debopti}/"
            rm -rf "$tmp_dir" "$tmp_tar"
        else
            err "❌ 远程同步失败。"
            return 1
        fi
    fi
    
    chmod +x "${INSTALL_DIR:-/opt/debopti}/deb_optimizer.sh"
    success "更新成功！正在重新载入脚本..."
    sleep 1
    exec "${INSTALL_DIR:-/opt/debopti}/deb_optimizer.sh"
}

# 2. 脚本完全卸载
script_uninstall() {
    warn "警告：此操作将移除所有已安装的优化逻辑、脚本资产及全局命令。"
    read -p "确定要彻底卸载 Debian Optimizer 吗？[y/N]: " confirm
    [[ ! "$confirm" =~ ^[Yy]$ ]] && return

    info "正在卸载脚本..."

    # 1. 移除全局命令
    rm -f "/usr/local/bin/debopti"

    # 2. 移除 Fish 环境增强
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
    rm -rf "/opt/debopti"
    
    success "卸载完成。系统已恢复至脚本安装前的状态（不包括已修改的内核/防火墙配置）。"
    exit 0
}
