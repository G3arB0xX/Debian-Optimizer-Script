#!/bin/bash
# =========================================================
# Ferron 高性能 Web 服务器自动化部署 (官方源标准版)
# =========================================================

# ----------------- 核心安装逻辑 -----------------
install_ferron() {
    info "正在通过 Ferron 官方标准仓库部署 Web 服务器..."

    # 1. 补齐仓库管理依赖
    safe_apt_install curl gnupg2 ca-certificates lsb-release debian-archive-keyring || return 1

    # 2. 注入官方签名密钥 (采用现代 keyring 隔离模式)
    info "正在添加 Ferron 官方 PGP 签名密钥..."
    local keyring="/usr/share/keyrings/ferron-keyring.gpg"
    local tmp_pgp="/tmp/ferron.pgp"
    if download_with_fallback "$tmp_pgp" "https://deb.ferron.sh/signing.pgp"; then
        gpg --dearmor -o "$keyring" --yes < "$tmp_pgp"
        rm -f "$tmp_pgp"
    else
        err "获取签名密钥失败，请检查网络连接。"
        return 1
    fi

    # 3. 配置 APT 软件源
    info "正在配置官方 APT 软件源..."
    local codename
    codename=$(lsb_release -cs)
    echo "deb [signed-by=$keyring] https://deb.ferron.sh $codename main" | tee /etc/apt/sources.list.d/ferron.list >/dev/null

    # 4. 执行安装
    info "同步包缓存并安装 Ferron..."
    apt-get update -yq >/dev/null 2>&1
    safe_apt_install ferron || {
        err "Ferron 软件包安装失败。可能是不支持当前系统发行版 ($codename)。"
        return 1
    }

    # 5. 配置文件目录结构标准化
    info "标准化配置文件路径至 /etc/ferron/config.kdl ..."
    mkdir -p /etc/ferron /etc/ferron/certs
    chmod 700 /etc/ferron/certs 2>/dev/null || true
    if [[ -f "/etc/ferron.kdl" ]]; then
        mv /etc/ferron.kdl /etc/ferron/config.kdl
    fi

    # 6. 注入全局优化配置块 (KDL 语法)
    info "注入全局优化配置块 (io_uring=false, stdout logging)..."
    local config="/etc/ferron/config.kdl"
    if [[ ! -f "$config" ]]; then
        render_template "templates/apps/ferron/config.kdl" "$config"
    else
        # 如果文件已存在，则在文件开头注入全局块 (幂等检查)
        if ! grep -q "io_uring #false" "$config"; then
            sed -i "1i * {\n    io_uring #false\n    log_stdout\n    error_log_stderr\n}\n" "$config"
        fi
    fi

    # 7. 基础环境初始化 (模拟 Nginx 404 页面)
    if [[ ! -d "/var/www/ferron" ]]; then
        mkdir -p /var/www/ferron
        render_template "templates/apps/ferron/index.html" "/var/www/ferron/index.html"
        chown -R ferron:ferron /var/www/ferron
    fi
    chown ferron:ferron /etc/ferron/certs 2>/dev/null || true

    # 8. 安全沙箱加固与路径纠偏 (Systemd Override)
    local ferron_bin
    ferron_bin=$(command -v ferron || echo "/usr/bin/ferron")
    
    render_template "templates/apps/ferron/ferron.service.override.conf" "-" "FERRON_BIN=$ferron_bin" | inject_service_override "ferron"

    if systemctl is-active --quiet ferron; then
        success "Ferron 已通过官方仓库成功安装并运行。"
        info "管理指令: systemctl [start|stop|restart|reload] ferron"
        info "配置文件: /etc/ferron/config.kdl"
        info "Web 根目录: /var/www/ferron"
        info "访问测试: http://$(curl -s4 ifconfig.me || echo 'localhost')"
    else
        warn "Ferron 已安装，但服务未正常启动，请检查 /etc/ferron/config.kdl 配置。"
    fi
}

# ----------------- 深度卸载逻辑 -----------------
uninstall_ferron() {
    info "正在执行 Ferron 深度清理程序..."

    # 1. 停止并移除服务
    systemctl stop ferron >/dev/null 2>&1
    systemctl disable ferron >/dev/null 2>&1

    # 2. 卸载软件包并清理残留配置
    apt-get purge -yq ferron >/dev/null 2>&1
    apt-get autoremove -yq >/dev/null 2>&1

    # 3. 清理仓库配置
    rm -f /etc/apt/sources.list.d/ferron.list
    rm -f /usr/share/keyrings/ferron-keyring.gpg
    apt-get update -yq >/dev/null 2>&1

    # 4. 暴力清理目录残留
    rm -rf /etc/ferron /etc/ferron.kdl /var/log/ferron /var/www/ferron /etc/systemd/system/ferron.service.d
    
    success "Ferron 官方组件及仓库配置已彻底移除。"
}
