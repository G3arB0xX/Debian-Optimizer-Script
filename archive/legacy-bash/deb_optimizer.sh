#!/bin/bash
set -euo pipefail

# =========================================================
# debopty - Debian 系统性能调优与服务自动化部署
# =========================================================
# 项目: debopty (Debian 系统调优与运维管理)
# 架构: 模块化解耦、原子化管理、现代防火墙、安全沙盒
# =========================================================

# 使用 readlink -f 确保从软链接启动时也能正确定位安装目录
if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
    SCRIPT_DIR="$(cd "$(dirname "$(readlink -f "${BASH_SOURCE[0]}")")" && pwd)"
else
    # 针对管道运行 (curl | bash) 或特殊环境的兜底逻辑
    SCRIPT_DIR="/opt/debopty"
fi
INSTALL_DIR="${INSTALL_DIR:-/opt/debopty}"
INSTALL_PATH="${INSTALL_DIR}/deb_optimizer.sh"
BIN_LINK="/usr/local/bin/debopty"

# ----------------- 运行权限拦截与自动提权 -----------------
# 优先处理权限问题，避免非 root 状态下触发配置目录创建失败
if [[ $EUID -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1; then
        abs_path="$(readlink -f "${BASH_SOURCE[0]:-$0}")"
        echo -e "\e[1;33m⚠️  当前非 root 权限，正在尝试通过 sudo 自动提权...\e[0m"
        # 使用 -E 保留环境变量，并确保以 bash 明确执行脚本
        exec sudo -E bash "$abs_path" "$@"
    else
        echo -e "\e[0;31m❌ 权限拦截：此脚本涉及底层内核参数与防火墙操作，且未检测到 sudo，请手动切换到 root 运行。\e[0m"
        exit 1
    fi
fi

# ----------------- 核心模块链式加载 -----------------
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/scripts/common.sh"
ensure_runtime_home
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
              "${SCRIPT_DIR}/scripts/apps/podman.sh" \
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
export PATH="${PATH}:/usr/local/go/bin:${HOME}/go/bin"

# =========================================================
# 项目同步与命令绑定
# =========================================================

_debopty_sync_shortcuts() {
    ln -sf "$INSTALL_PATH" "$BIN_LINK"

    if [[ -L "/usr/local/bin/debopti" ]]; then
        local legacy_target=""
        legacy_target="$(readlink -f "/usr/local/bin/debopti" 2>/dev/null || true)"
        if [[ "$legacy_target" == *"deb_optimizer.sh" || ! -e "/usr/local/bin/debopti" ]]; then
            rm -f "/usr/local/bin/debopti"
        fi
    fi
    rm -f "/etc/profile.d/debopti.sh" "/etc/fish/conf.d/debopti.fish"

    # 2. 注入全局 Bash 环境
    render_template "templates/common/debopty.sh" "/etc/profile.d/debopty.sh" "BIN_LINK=$BIN_LINK"
    chmod +x /etc/profile.d/debopty.sh

    # 3. 注入 Fish 环境 (如果 Fish 已安装)
    if command -v fish >/dev/null 2>&1; then
        render_template "templates/common/debopty.fish" "/etc/fish/conf.d/debopty.fish" "BIN_LINK=$BIN_LINK"
    fi
}

self_install() {
    # 确保目录存在
    mkdir -p "$INSTALL_DIR"

    # 路径一致性检查：如果在安装路径运行，则仅进行环境自愈检查
    if [[ "$(readlink -f "$0")" == "$INSTALL_PATH" ]]; then
        if [[ ! -L "$BIN_LINK" || ! -f "/etc/profile.d/debopty.sh" || -L "/usr/local/bin/debopti" || -f "/etc/profile.d/debopti.sh" ]]; then
            _debopty_sync_shortcuts
        fi
        return 0
    fi

    # 状态标记检查
    if [[ "${INSTALLED:-}" == "true" && -f "$INSTALL_PATH" ]]; then
        if [[ ! -L "$BIN_LINK" || ! -f "/etc/profile.d/debopty.sh" || -L "/usr/local/bin/debopti" || -f "/etc/profile.d/debopti.sh" ]]; then
            _debopty_sync_shortcuts
        fi
        return 0
    fi

    info "正在执行自动化自举安装 (同步项目至 $INSTALL_DIR)..."
    
    local current_src_dir="$(cd "$(dirname "$0")" && pwd)"
    # 使用 rsync 或更安全的 cp 逻辑，避免递归错误
    cp -rf "${current_src_dir}/"* "$INSTALL_DIR/" 2>/dev/null || true
    
    # 清理非生产文件
    rm -rf "${INSTALL_DIR}/.git" "${INSTALL_DIR}/.jj" "${INSTALL_DIR}/.github" "${INSTALL_DIR}/.jj_backup" \
        "${INSTALL_DIR}/.cursor" "${INSTALL_DIR}/tools" "${INSTALL_DIR}/governance" "${INSTALL_DIR}/instructions" "${INSTALL_DIR}/tests" \
        "${INSTALL_DIR}/rust" "${INSTALL_DIR}/AGENTS.md" "${INSTALL_DIR}/CLAUDE.md" "${INSTALL_DIR}/GEMINI.md" \
        "${INSTALL_DIR}/Makefile" "${INSTALL_DIR}/.shellcheckrc" "${INSTALL_DIR}/.editorconfig" \
        "${INSTALL_DIR}/.gitignore"
    find "${INSTALL_DIR}" -type f -name 'INDEX.md' -delete 2>/dev/null || true
    
    chmod +x "$INSTALL_PATH"
    
    # 1. 创建二进制软链接与 Shell 快捷命令
    _debopty_sync_shortcuts
    
    save_project_config "INSTALLED" "true"
    _debopti_persist_identity "${DEBOPTI_VERSION:-}" "${DEBOPTI_BUILD:-${VERSION_ID:-}}"
    info "安装成功。后续您可以直接在终端输入: ${CYAN}debopty${NC}"
    sleep 1
    
    # 进程接管：安装后跳转到安装路径执行，确保后续模块引用路径一致
    exec "$INSTALL_PATH" "$@"
}

# [内部] 启动时版本检查
check_startup_update() {
    # 1. 仅在非开发模式（即在 /opt/debopty 运行）时检查
    if [[ "$(readlink -f "$0")" != "$INSTALL_PATH" ]]; then
        return 0
    fi
    
    # 2. 检查是否处于更新重启后的状态，避免死循环
    if [[ "${IN_UPDATE_RESTART:-}" == "true" ]]; then
        return 0
    fi

    # 3. 尝试静默检查版本 (设置超时避免阻塞，实时拉取避免 CDN 缓存误判)
    local remote_id installed_id remote_ver remote_build inst_ver inst_build
    installed_id="$(get_installed_identity)"
    inst_ver="${installed_id%%|*}"
    inst_build="${installed_id#*|}"
    [[ -n "$inst_build" ]] || inst_build="${VERSION_ID:-}"
    remote_id="$(fetch_remote_identity 2>/dev/null || echo "")"
    remote_ver="${remote_id%%|*}"
    remote_build="${remote_id#*|}"

    if debopti_needs_update "$inst_build" "$remote_build"; then
        # 标记重启状态并跳转更新（提示由 script_update 统一输出，避免重复「检测到新版本」）
        export IN_UPDATE_RESTART="true"
        script_update
    fi
    return 0
}

# =========================================================
# 执行引擎
# =========================================================

_debopty_maybe_exec_rust() {
    [[ "${DEBOPTY_ENGINE:-bash}" == "rust" ]] || return 0
    [[ "${DEBOPTY_FROM_RUST:-0}" != "1" ]] || return 0

    local rs_bin="" candidate
    for candidate in \
        "${DEBOPTY_RS_BIN:-}" \
        "${INSTALL_DIR}/bin/debopty" \
        "/usr/local/bin/debopty-rs"; do
        if [[ -n "$candidate" && -f "$candidate" && -x "$candidate" ]]; then
            rs_bin="$candidate"
            break
        fi
    done

    if [[ -n "$rs_bin" ]]; then
        exec "$rs_bin" "$@"
    fi

    if declare -f warn >/dev/null 2>&1; then
        warn "已请求 DEBOPTY_ENGINE=rust，但未找到可执行的 Rust 二进制文件，继续使用 Bash 引擎。"
    else
        echo -e "\e[1;33m⚠️  已请求 DEBOPTY_ENGINE=rust，但未找到可执行的 Rust 二进制文件，继续使用 Bash 引擎。\e[0m" >&2
    fi
    return 0
}

# 0. 可选 Rust 引擎共存分发 (默认保持 Bash 行为不变)
_debopty_maybe_exec_rust "$@"

# 1. 执行项目自举
self_install "$@"

# 2. 启动版本检查
check_startup_update

# 3. DNS 健康检查 (第一阶段：地区未知，仅用 1.1.1.1 临时救活)
check_and_fix_dns initial

# 4. 网络归属地自动识别
global_netcheck

# 5. DNS 健康检查 (第二阶段：根据地区与 IP 栈完整校准)
check_and_fix_dns calibrate

# 6. 自动修补旧版防火墙遗留的 IPv6 断网问题
if declare -f patch_nftables_ndp > /dev/null; then
    patch_nftables_ndp
fi

# 6.5 DevOps SOT 启动钩子：为新用户同步 Fish / Micro / Yazi 只读软链接
if declare -f maybe_sync_devops_sot_on_startup > /dev/null; then
    maybe_sync_devops_sot_on_startup
fi

# 7. 首次运行强制优化流程
if [[ "${BASE_OPTIMIZED:-}" != "true" ]]; then
    info "检测到系统未经过基础调优，启动首次优化任务..."
    check_ssh_security || exit 1
    run_base_optimization
    info "系统基础环境已加固，正在加载管理面板..."
    sleep 2
fi

# 4. 进入交互式 TUI (死循环保护)
export IN_TUI="true"
while true; do
    show_main_menu || break
done

info "脚本执行结束。祝您运维愉快！"
unset IN_TUI
exit 0
