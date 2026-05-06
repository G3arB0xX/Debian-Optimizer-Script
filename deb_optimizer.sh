#!/bin/bash
set -euo pipefail

# =========================================================
# Debian 系统性能调优与服务自动化部署面板 (模块化生产版)
# =========================================================
# 编写准则: VIBEINSTRCT.md
# 设计理念: 模块化解耦、原子化管理、现代防火墙、零信任安全
# =========================================================

# ----------------- 基础环境侦测 -----------------
# 动态计算脚本运行路径，确保模块加载不受执行路径影响
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# 加载持久化配置状态
INIT_FLAG="/etc/servopti.conf"
if [[ -f "$INIT_FLAG" ]]; then
    # shellcheck disable=SC1090
    source "$INIT_FLAG"
fi

# 初始化状态变量
IS_CN_REGION="${IS_CN_REGION:-}"
BASE_OPTIMIZED="${BASE_OPTIMIZED:-}"
INSTALLED="${INSTALLED:-}"

# ----------------- 核心模块链式加载 -----------------
# 模块间存在依赖关系：common/network 为底层，tui 为展示层，apps 为功能层
for module in "${SCRIPT_DIR}/scripts/common.sh" \
              "${SCRIPT_DIR}/scripts/network.sh" \
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

# ----------------- 运行权限拦截 -----------------
if [[ $EUID -ne 0 ]]; then
   die "权限拦截：此脚本涉及底层内核参数与防火墙操作，必须以 root 权限运行。"
fi

# =========================================================
# 自动化项目同步与命令绑定引擎
# =========================================================
INSTALL_DIR="/opt/debopti"
INSTALL_PATH="${INSTALL_DIR}/deb_optimizer.sh"
BIN_LINK="/usr/local/bin/debopti"

self_install() {
    # 路径一致性检查：如果在安装路径运行，则视为环境正常
    if [[ "$(readlink -f "$0")" == "$INSTALL_PATH" ]]; then
        return
    fi

    # 状态标记检查：避免无意义的重复安装
    if [[ "${INSTALLED:-}" == "true" && -f "$INSTALL_PATH" ]]; then
        return
    fi

    info "正在执行自动化自举安装 (同步项目至 $INSTALL_DIR)..."
    
    mkdir -p "$INSTALL_DIR"
    
    # 采用递归克隆模式同步整个项目结构
    local current_src_dir="$(cd "$(dirname "$0")" && pwd)"
    # 排除版本管理目录，仅同步代码资产
    cp -r "${current_src_dir}/"* "$INSTALL_DIR/" 2>/dev/null || true
    
    chmod +x "$INSTALL_PATH"
    
    # 创建全局系统命令，支持在任何 Shell 下直接输入 'debopti'
    if [[ -L "$BIN_LINK" || -f "$BIN_LINK" ]]; then
        rm -f "$BIN_LINK"
    fi
    ln -sf "$INSTALL_PATH" "$BIN_LINK"
    
    # 持久化安装标记
    touch "$INIT_FLAG"
    sed -i '/INSTALLED/d' "$INIT_FLAG" 2>/dev/null
    echo "INSTALLED=\"true\"" >> "$INIT_FLAG"
    
    info "✅ 安装成功！后续请直接执行: debopti"
    sleep 1
    
    # 进程接管：无缝跳转到正式安装路径执行
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

# 4. 进入交互式 TUI
show_main_menu

info "脚本执行结束。祝您运维愉快！"
exit 0
