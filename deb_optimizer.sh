#!/bin/bash
set -euo pipefail

# =========================================================
# Debian 系统性能调优与服务自动化部署
# =========================================================
# 准则: VIBEINSTRCT.md
# 架构: 模块化解耦、原子化管理、现代防火墙、安全沙盒
# =========================================================

# ----------------- 基础环境侦测 -----------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# ----------------- 运行权限拦截与自动提权 -----------------
# 优先处理权限问题，避免非 root 状态下触发配置目录创建失败
if [[ $EUID -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1; then
        abs_path=$(readlink -f "$0")
        echo -e "\e[1;33m⚠️  当前非 root 权限，正在尝试通过 sudo 自动提权...\e[0m"
        exec sudo -p " [sudo] %u 的密码: " "$abs_path" "$@"
    else
        echo -e "\e[0;31m❌ 权限拦截：此脚本涉及底层内核参数与防火墙操作，且未检测到 sudo，请手动切换到 root 运行。\e[0m"
        exit 1
    fi
fi

# ----------------- 核心模块链式加载 -----------------
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/scripts/common.sh"
load_project_config

# 初始化状态变量
IS_CN_REGION="${IS_CN_REGION:-}"
BASE_OPTIMIZED="${BASE_OPTIMIZED:-}"
INSTALLED="${INSTALLED:-}"

for module in "${SCRIPT_DIR}/scripts/network.sh" \
              "${SCRIPT_DIR}/scripts/system.sh" \
              "${SCRIPT_DIR}/scripts/security.sh" \
              "${SCRIPT_DIR}/scripts/tui.sh" \
              "${SCRIPT_DIR}/scripts/apps/xray.sh" \
              "${SCRIPT_DIR}/scripts/apps/easytier.sh" \
              "${SCRIPT_DIR}/scripts/apps/tailscale.sh" \
              "${SCRIPT_DIR}/scripts/apps/warp.sh" \
              "${SCRIPT_DIR}/scripts/apps/docker.sh" \
              "${SCRIPT_DIR}/scripts/apps/realm.sh" \
              "${SCRIPT_DIR}/scripts/apps/ferron.sh" \
              "${SCRIPT_DIR}/scripts/apps/rust.sh" \
              "${SCRIPT_DIR}/scripts/apps/golang.sh" \
              "${SCRIPT_DIR}/scripts/apps/freship.sh" \
              "${SCRIPT_DIR}/scripts/apps/devops.sh"; do
    if [[ -f "$module" ]]; then
        # shellcheck disable=SC1090
        source "$module"
    else
        echo "错误: 关键组件丢失: $module"
        exit 1
    fi
done

# ----------------- 全局环境变量注入 -----------------
export PATH=$PATH:/usr/local/go/bin:$HOME/go/bin

# =========================================================
# 项目同步与命令绑定
# =========================================================
INSTALL_DIR="/opt/debopti"
INSTALL_PATH="${INSTALL_DIR}/deb_optimizer.sh"
BIN_LINK="/usr/local/bin/debopti"

self_install() {
    # 路径一致性检查：如果在安装路径运行，则进行环境自愈检查
    if [[ "$(readlink -f "$0")" == "$INSTALL_PATH" ]]; then
        [[ ! -L "$BIN_LINK" ]] && ln -sf "$INSTALL_PATH" "$BIN_LINK"
        return
    fi

    # 状态标记检查
    if [[ "${INSTALLED:-}" == "true" && -f "$INSTALL_PATH" ]]; then
        [[ ! -L "$BIN_LINK" ]] && ln -sf "$INSTALL_PATH" "$BIN_LINK"
        return
    fi

    info "正在执行自动化自举安装 (同步项目至 $INSTALL_DIR)..."
    
    mkdir -p "$INSTALL_DIR"
    
    local current_src_dir="$(cd "$(dirname "$0")" && pwd)"
    cp -r "${current_src_dir}/"* "$INSTALL_DIR/" 2>/dev/null || true
    rm -rf "${INSTALL_DIR}/.git" "${INSTALL_DIR}/.jj" "${INSTALL_DIR}/.github" "${INSTALL_DIR}/.jj_backup"
    
    chmod +x "$INSTALL_PATH"
    
    # 1. 创建二进制软链接
    ln -sf "$INSTALL_PATH" "$BIN_LINK"
    
    # 2. 注入全局 Bash Alias
    echo "alias debopti='$INSTALL_PATH'" > /etc/profile.d/debopti.sh
    chmod +x /etc/profile.d/debopti.sh
    
    # 3. 注入 Fish Abbreviation (如果 Fish 已安装)
    if command -v fish >/dev/null 2>&1; then
        mkdir -p /etc/fish/conf.d/
        echo "abbr -a debopti '$INSTALL_PATH'" > /etc/fish/conf.d/debopti.fish
    fi
    
    save_project_config "INSTALLED" "true"
    info "安装成功。后续执行: debopti"
    sleep 1
    
    # 进程接管
    exec "$INSTALL_PATH" "$@"
}

# =========================================================
# 执行引擎
# =========================================================

# 1. 执行项目自举
self_install "$@"

# 2. 网络归属地自动识别
global_netcheck

# 3. 首次运行强制优化流程
if [[ "${BASE_OPTIMIZED:-}" != "true" ]]; then
    info "检测到系统未经过基础调优，启动首次优化任务..."
    check_ssh_security || exit 1
    run_base_optimization
    info "系统基础环境已加固，正在加载管理面板..."
    sleep 2
fi

# 4. 进入交互式 TUI (死循环保护)
while true; do
    show_main_menu || break
done

info "脚本执行结束。祝您运维愉快！"
exit 0
