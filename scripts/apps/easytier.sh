#!/bin/bash
# =========================================================
# Easytier 虚拟组网组件管理
# =========================================================

install_easytier() {
    info "正在安装/更新 Easytier 跨平台组网引擎..."
    
    # 依赖检查
    if ! command -v unzip >/dev/null 2>&1; then
        info "补齐解压工具 (unzip)..."
        apt-get update -yq >/dev/null 2>&1
        apt-get install -yq unzip >/dev/null 2>&1
    fi

    # 获取官方安装脚本
    download_with_fallback "/tmp/easytier-install.sh" "https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh" || return 1
    
    # 网络加速适配
    local proxy_args="--no-gh-proxy"
    [[ "$IS_CN_REGION" == "true" ]] && proxy_args="--gh-proxy https://ghfast.top/"
    
    # 判定：以 /opt/easytier/easytier-core 为准（与官方 update 前置检查一致）
    # 已安装 → 官方 update；未安装 → 官方 install
    # 更新路径不 enable/disable easytier@，自启与运行状态由官方 update 按更新前实例保持
    local is_fresh_install="false"
    if [[ -f "/opt/easytier/easytier-core" ]]; then
        info "检测到已安装版本，调用官方脚本 update..."
        bash /tmp/easytier-install.sh update $proxy_args || return 1
    else
        is_fresh_install="true"
        info "执行全新安装部署（官方 install）..."
        bash /tmp/easytier-install.sh install $proxy_args || return 1
    fi

    # --- 首次安装：关闭开机自启（官方默认 enable easytier@default）---
    if [[ "$is_fresh_install" == "true" ]]; then
        info "正在禁用 Easytier 开机自启（easytier / easytier@）..."
        systemctl stop easytier easytier@default 2>/dev/null || true
        systemctl stop "easytier@*" 2>/dev/null || true
        systemctl disable easytier 2>/dev/null || true
        systemctl disable easytier@default 2>/dev/null || true
        systemctl disable "easytier@*" 2>/dev/null || true
    fi

    # --- CLI 加入 PATH（Bash profile.d + Fish SOT，幂等）---
    render_template "templates/apps/easytier/profile.d/debopti-easytier.sh" "/etc/profile.d/debopti-easytier.sh"
    chmod 644 /etc/profile.d/debopti-easytier.sh
    update_fish_path "/opt/easytier"
    
    # --- 安全沙箱加固 (Systemd Override) ---
    inject_service_override "easytier@" << EOF
[Service]
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
NoNewPrivileges=true
# Easytier 需要网卡管理能力
CapabilityBoundingSet=CAP_NET_ADMIN CAP_NET_BIND_SERVICE CAP_NET_RAW
EOF
    
    # 防火墙 P2P 端口自动放行 (nftables)
    # 默认使用 11010 - 11015 范围以支持多实例并发打洞
    add_fw_rule "11010-11015" "tcp/udp" "Easytier_P2P"
    
    success "Easytier 操作完成。"
    info "主配置文件路径: /opt/easytier/config/default.conf"
    if [[ "$is_fresh_install" == "true" ]]; then
        info "已默认关闭开机自启；启动实例: systemctl start easytier@default"
        info "CLI 已加入 PATH（easytier-cli / easytier-core）；新登录 shell 生效。"
    fi
}

uninstall_easytier() {
    info "准备彻底卸载 Easytier..."
    
    if [[ ! -f "/tmp/easytier-install.sh" ]]; then
        download_with_fallback "/tmp/easytier-install.sh" "https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh" || return 1
    fi
    # 调用官方卸载逻辑
    bash /tmp/easytier-install.sh uninstall >/dev/null 2>&1

    info "执行深度清理 (残留配置与 Systemd 单元)..."
    systemctl stop easytier >/dev/null 2>&1
    systemctl disable easytier >/dev/null 2>&1
    rm -rf /etc/systemd/system/easytier*
    rm -rf /etc/systemd/system/easytier@.service.d
    systemctl daemon-reload

    # 还原 PATH 配置
    rm -f /etc/profile.d/debopti-easytier.sh
    remove_fish_path "/opt/easytier"
    
    # 彻底抹除二进制与安装目录
    rm -rf /usr/bin/easytier-core /usr/bin/easytier-cli /usr/local/bin/easytier-core /opt/easytier /opt/easytier-core
    rm -f /usr/sbin/easytier-core /usr/sbin/easytier-cli
    
    # 清理防火墙规则
    [[ -f "${NFT_CONF_DIR}/Easytier_P2P.nft" ]] && rm -f "${NFT_CONF_DIR}/Easytier_P2P.nft" && nft -f /etc/nftables.conf

    success "Easytier 已从系统完全移除。"
}
