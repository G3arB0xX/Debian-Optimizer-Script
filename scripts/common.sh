#!/bin/bash
# =========================================================
# 通用工具模块 (标准 UI 与日志规范)
# =========================================================

# ----------------- 终端视觉定义 -----------------
# 使用标准的 ANSI 转义码，确保在不同 SSH 客户端下的兼容性
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# ----------------- 结构化日志函数 -----------------

# [INFO] 用于显示正常运行的状态更新
info() { echo -e "${GREEN}[INFO] $1${NC}"; }

# [WARN] 用于显示需要用户注意的非致命问题或逻辑跳过
warn() { echo -e "${YELLOW}[WARN] $1${NC}"; }

# [ERROR] 用于显示操作失败但不会中断脚本运行的错误
err()  { echo -e "${RED}[ERROR] $1${NC}"; }

# [FATAL] 致命错误，输出日志并以状态码 1 彻底退出
die()  { echo -e "${RED}[FATAL] $1${NC}"; exit 1; }

# ----------------- 交互逻辑 -----------------

# 暂停函数：在 TUI 模式下防止日志闪现，给予 PM 阅览报错的时间
# 采用非阻塞读取，支持任意键继续
pause() {
    echo -e "\n${YELLOW}>>> 操作执行完毕。请阅读上方日志，按任意键返回菜单...${NC}"
    # -n 1: 仅读取一个字符; -s: 静默不回显; -r: 防止反斜杠转义
    read -n 1 -s -r -p ""
}
# ----------------- 系统与环境状态 -----------------

# 获取系统中的第一个普通用户 (UID >= 1000, 排除 nobody)
get_normal_user() {
    local user
    user=$(awk -F: '$3 >= 1000 && $1 != "nobody" {print $1}' /etc/passwd | head -n 1)
    echo "$user"
}

# 动态配置 Fish 环境变量
# 参数: $1=变量名, $2=变量值
update_fish_env() {
    local var_name=$1
    local var_value=$2
    
    # 检查 fish 是否安装
    if ! command -v fish >/dev/null 2>&1; then
        return 0
    fi

    info "正在同步 Fish 环境变量: $var_name ..."

    local users=()
    users+=("root")
    local normal_user
    normal_user=$(get_normal_user)
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    for user in "${users[@]}"; do
        local user_home
        user_home=$(eval echo "~$user")
        local conf_d="$user_home/.config/fish/conf.d"
        local env_file="$conf_d/debopti_vars.fish"

        mkdir -p "$conf_d"
        
        # 幂等性写入: 如果变量已存在则更新，不存在则追加
        if grep -q "set -gx $var_name " "$env_file" 2>/dev/null; then
            sed -i "s|set -gx $var_name .*|set -gx $var_name $var_value|" "$env_file"
        else
            echo "set -gx $var_name $var_value" >> "$env_file"
        fi
        
        # 修复权限
        chown -R "$user:$user" "$user_home/.config/fish" 2>/dev/null || true
        
        # 验证配置文件正确性
        if ! sudo -u "$user" fish -n "$user_home/.config/fish/config.fish" 2>/dev/null; then
            warn "用户 $user 的 Fish 配置校验失败，正在回滚环境变量修改..."
            sed -i "/set -gx $var_name /d" "$env_file"
        fi
    done
}

# 动态配置 Fish PATH
# 参数: $1=路径
update_fish_path() {
    local target_path=$1
    
    # 检查 fish 是否安装
    if ! command -v fish >/dev/null 2>&1; then
        return 0
    fi

    info "正在同步 Fish PATH: $target_path ..."

    local users=()
    users+=("root")
    local normal_user
    normal_user=$(get_normal_user)
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    for user in "${users[@]}"; do
        local user_home
        user_home=$(eval echo "~$user")
        local conf_d="$user_home/.config/fish/conf.d"
        local path_file="$conf_d/debopti_path.fish"

        mkdir -p "$conf_d"
        
        # 幂等性写入: 使用 fish_add_path (fish 3.2+)
        # 如果 fish 版本太旧，降级使用 set -gx PATH
        local fish_version=$(fish --version | awk '{print $3}')
        if [[ $(echo "$fish_version 3.2" | awk '{print ($1 >= $2)}') -eq 1 ]]; then
            if ! grep -q "fish_add_path $target_path" "$path_file" 2>/dev/null; then
                echo "fish_add_path $target_path" >> "$path_file"
            fi
        else
            if ! grep -q "contains $target_path \$PATH" "$path_file" 2>/dev/null; then
                echo "if not contains $target_path \$PATH; set -gx PATH \$PATH $target_path; end" >> "$path_file"
            fi
        fi
        
        # 修复权限
        chown -R "$user:$user" "$user_home/.config/fish" 2>/dev/null || true
    done
}

# 移除 Fish 环境变量
remove_fish_env() {
    local var_name=$1
    if ! command -v fish >/dev/null 2>&1; then return 0; end

    local users=("root")
    local normal_user=$(get_normal_user)
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    for user in "${users[@]}"; do
        local user_home=$(eval echo "~$user")
        local env_file="$user_home/.config/fish/conf.d/debopti_vars.fish"
        [[ -f "$env_file" ]] && sed -i "/set -gx $var_name /d" "$env_file"
    done
}

# 移除 Fish PATH
remove_fish_path() {
    local target_path=$1
    if ! command -v fish >/dev/null 2>&1; then return 0; end

    local users=("root")
    local normal_user=$(get_normal_user)
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    for user in "${users[@]}"; do
        local user_home=$(eval echo "~$user")
        local path_file="$user_home/.config/fish/conf.d/debopti_path.fish"
        [[ -f "$path_file" ]] && sed -i "s|.*$target_path.*||g" "$path_file"
    done
}
