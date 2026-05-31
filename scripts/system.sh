#!/bin/bash
# =========================================================
# 系统级底层调优模块 (内核、协议栈与系统限制)
# =========================================================

# ----------------- 基础环境优化 -----------------
setup_base() {
    info "正在优化 APT 镜像源与系统基础组件..."
    
    # 自动备份官方源 (处理多种格式)
    if [[ -f /etc/apt/sources.list ]]; then
        [[ ! -f /etc/apt/sources.list.bak ]] && cp /etc/apt/sources.list /etc/apt/sources.list.bak
    fi
    
    export DEBIAN_FRONTEND=noninteractive
    local debian_ver
    debian_ver=$(cut -d. -f1 /etc/debian_version 2>/dev/null || echo "0")

    # 针对 Debian 10 (Buster) EOL 的特殊处理：切换至 Archive 存档源
    if [[ "$debian_ver" == "10" ]]; then
        info "检测到 Debian 10 (Buster) 已 EOL，正在切换至 Archive 存档源..."
        if [[ "$IS_CN_REGION" == "true" ]]; then
            render_template "templates/system/sources.list.buster.cn" "/etc/apt/sources.list"
        else
            render_template "templates/system/sources.list.buster.global" "/etc/apt/sources.list"
        fi
    fi

    # 破除“鸡生蛋”死锁：预装 CA 证书以支持后续的 HTTPS 请求
    if ! dpkg -s ca-certificates >/dev/null 2>&1; then
        info "预装 CA 证书以兼容 HTTPS 源..."
        apt-get update -yq >/dev/null 2>&1 || true
        apt-get install -yq ca-certificates >/dev/null 2>&1 || warn "CA 证书安装失败，HTTPS 源可能不可用"
    fi

    # 国内环境自适应切换到 TUNA 镜像站 (Debian 11+)
    if [[ "$IS_CN_REGION" == "true" && "$debian_ver" != "10" ]]; then
        if [[ -f /etc/apt/sources.list ]]; then
            sed -i 's/deb.debian.org/mirrors.tuna.tsinghua.edu.cn/g' /etc/apt/sources.list
            sed -i 's/security.debian.org/mirrors.tuna.tsinghua.edu.cn/g' /etc/apt/sources.list
        fi
    fi

    # 强制升级源协议到更安全的 HTTPS (如果存在 sources.list)
    if [[ "$debian_ver" == "10" ]]; then
        [[ "$IS_CN_REGION" == "true" ]] && sed -i 's|http://|https://|g' /etc/apt/sources.list
    else
        [[ -f /etc/apt/sources.list ]] && sed -i 's|http://|https://|g' /etc/apt/sources.list
    fi

    info "同步包缓存并补齐基础系统工具..."
    # 跨版本兼容处理：
    # - dnsutils 在 Debian 13 (Trixie) 已更名为 bind9-dnsutils
    # - jq 是 freship.sh 的运行时依赖，归入基础工具
    local dns_pkg="dnsutils"
    if [[ "$debian_ver" -ge 13 ]]; then
        dns_pkg="bind9-dnsutils"
    fi

    local base_tools=(
        "curl" "wget" "gnupg" "lsb-release" "procps"
        "unzip" "tar" "openssl" "git" "logrotate"
        "whois" "$dns_pkg" "net-tools" "jq" "sudo"
    )
    safe_apt_install "${base_tools[@]}" || warn "部分基础工具安装受阻，请检查软件源。"
    apt-get upgrade -yq && apt-get autoremove -yq
}

# ----------------- 内核自适应更换 -----------------
setup_kernel() {
    info "检查系统内核架构..."
    local current_kernel=$(uname -r)
    
    # Cloud 内核针对 KVM/Xen 环境去除了物理驱动，启动更快，内存占用更低
    if echo "$current_kernel" | grep -q "cloud"; then
        info "当前已是 Cloud 专用内核，无需更换。"
    else
        echo -e "${YELLOW}检测到当前为物理机内核，建议更换为 Cloud 内核以降低内存开销。${NC}"
        local choice
        if [[ -n "${CI:-}" || ! -t 0 ]]; then
            info "CI/非交互模式：自动跳过内核更换。"
            choice="n"
        else
            read -p "是否更换为 Cloud 内核并自动清理旧内核？[y/N]: " choice
        fi

        if [[ "$choice" =~ ^[Yy]$ ]]; then
            apt-get install -yq linux-image-cloud-amd64 linux-headers-cloud-amd64 || die "内核下载失败！"
            update-grub
            # 自动清理除了当前和 Cloud 以外的所有冗余内核，释放 /boot 空间
            local old_kernels=$(dpkg -l | grep linux-image | awk '{print $2}' | grep -v "cloud" | grep -v "$current_kernel" || true)
            [[ -n "$old_kernels" ]] && apt-get purge -yq $old_kernels
            success "内核更换成功，重启后生效。"
        fi
    fi
}

# ----------------- TCP 协议栈调优 (BBR) -----------------
setup_sysctl() {
    info "下发建站级 Sysctl 协议栈优化参数..."
    local conf_file="/etc/sysctl.d/99-debopti-optimize.conf"
    
    render_template "templates/system/99-debopti-optimize.conf" "$conf_file"
    sysctl --system > /dev/null 2>&1 || warn "内核参数应用受限 (常见于容器环境)，已跳过。"
    success "TCP 协议栈优化已尝试激活。"
}

# ----------------- 系统资源限制优化 -----------------
setup_limits() {
    info "解除系统用户级最大文件句柄限制 (nofile)..."
    local limits_conf="/etc/security/limits.d/99-debopti-nofile.conf"
    
    render_template "templates/system/99-debopti-nofile.conf" "$limits_conf"
    # 同时同步 Systemd 的全局限制，确保通过 systemctl 启动的服务也受惠
    set_conf_value "/etc/systemd/system.conf" "DefaultLimitNOFILE" "1048576"
    set_conf_value "/etc/systemd/user.conf" "DefaultLimitNOFILE" "1048576"
    success "文件句柄限制已解除 (需重新登录生效)。"
}

# ----------------- 内存与虚拟内存管理 -----------------
setup_memory() {
    info "正在配置内存优化策略 (ZRAM & Swap)..."
    local mem_mb=$(( $(grep MemTotal /proc/meminfo | awk '{print $2}') / 1024 ))
    
    # ZRAM：使用 CPU 计算换取内存空间，适合小内存 VPS (推荐)
    local zram_choice
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：自动启用 ZRAM 内存压缩。"
        zram_choice="y"
    else
        read -p "是否启用 ZRAM 内存压缩？(建议 2G 以下内存开启) [y/N]: " zram_choice
    fi

    if [[ "$zram_choice" =~ ^[Yy]$ ]]; then
        apt-get install -yq zram-tools > /dev/null
        # 配置 50% 内存作为 ZRAM，使用高性能 zstd 算法
        render_template "templates/system/zramswap" "/etc/default/zramswap"
        systemctl restart zramswap >/dev/null 2>&1 || warn "ZRAM 启动失败 (可能由于缺少内核支持)。"
        success "ZRAM 优化指令已下发。"
    fi
    
    # 物理 Swap 文件兜底
    local swap_choice
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：自动创建物理 Swap 交换文件。"
        swap_choice="y"
    else
        read -p "是否创建物理 Swap 交换文件？[y/N]: " swap_choice
    fi

    if [[ "$swap_choice" =~ ^[Yy]$ ]]; then
        if grep -q "/swapfile" /proc/swaps; then
            info "Swap 文件已存在，跳过。"
        else
            local swap_size=$(( mem_mb * 2 ))
            info "正在创建 ${swap_size}MB Swap 文件..."
            fallocate -l ${swap_size}M /swapfile 2>/dev/null || dd if=/dev/zero of=/swapfile bs=1M count=${swap_size} status=progress
            chmod 600 /swapfile
            mkswap /swapfile >/dev/null 2>&1
            if swapon /swapfile 2>/dev/null; then
                [[ ! $(grep "/swapfile" /etc/fstab) ]] && echo "/swapfile none swap sw 0 0" >> /etc/fstab
                success "Swap 挂载成功。"
            else
                warn "环境不支持挂载 Swap (常见于部分 LXC 容器)，已清理。"
                rm -f /swapfile
            fi
        fi
    fi
}

# ----------------- 日志轮转与清理 -----------------
setup_logrotate() {
    info "配置 Logrotate 日志按天轮转，防止磁盘撑爆..."
    # 强制将每周轮换改为每日，保留 7 天副本并开启压缩
    set_conf_value "/etc/logrotate.conf" "daily" "" ""
    set_conf_value "/etc/logrotate.conf" "rotate" "7"
    set_conf_value "/etc/logrotate.conf" "compress" "" ""
    success "日志轮转配置更新。"
}

# ----------------- 内存极限瘦身 (Low Memory Optimization) -----------------
# 针对 1G 及以下内存 VPS 的深度优化，削减冗余进程与日志开销
setup_low_memory_optimization() {
    info "正在执行系统级极限瘦身优化 (针对低配 VPS)..."

    # 1. 削减 TTY 终端数量 (保留 2 个以防万一)
    info "削减冗余 TTY 终端进程..."
    if [[ -f /etc/systemd/logind.conf ]]; then
        set_conf_value "/etc/systemd/logind.conf" "NAutoVTs" "2"
        set_conf_value "/etc/systemd/logind.conf" "ReserveVT" "2"
        systemctl restart systemd-logind >/dev/null 2>&1 || true
    fi

    # 2. 移除重复的日志系统 (rsyslog)
    if dpkg -s rsyslog >/dev/null 2>&1; then
        info "检测到 rsyslog，正在卸载以释放内存 (改由 journald 接管)..."
        apt-get purge -yq rsyslog >/dev/null 2>&1
    fi

    # 3. 限制 Systemd Journal 日志的体量
    info "限制 Journald 日志内存与磁盘配额..."
    local journal_conf="/etc/systemd/journald.conf"
    if [[ -f "$journal_conf" ]]; then
        set_conf_value "$journal_conf" "SystemMaxUse" "200M"
        set_conf_value "$journal_conf" "RuntimeMaxUse" "10M"
        systemctl restart systemd-journald >/dev/null 2>&1 || true
    fi

    # 4. 裁剪系统冗余服务 (可选)
    local service_choice
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：自动屏蔽冗余服务。"
        service_choice="y"
    else
        echo -e "${YELLOW}是否屏蔽系统冗余服务 (ModemManager, Avahi, Bluetooth 等)？[y/N]: ${NC}"
        read -p "" service_choice
    fi

    if [[ "$service_choice" =~ ^[Yy]$ ]]; then
        info "正在屏蔽冗余服务..."
        local services=("ModemManager" "avahi-daemon" "bluetooth" "cups" "pnmos")
        for svc in "${services[@]}"; do
            if systemctl list-unit-files | grep -q "^${svc}.service"; then
                systemctl stop "$svc" >/dev/null 2>&1 || true
                systemctl mask "$svc" >/dev/null 2>&1 || true
                info "已屏蔽服务: $svc"
            fi
        done
    fi

    success "极限瘦身优化已完成。"
}

# ----------------- 时区与时间同步 -----------------
setup_timezone() {
    info "校准系统时区与时间同步..."
    # 强制设置为 Asia/Shanghai，确保日志时间线一致
    timedatectl set-timezone Asia/Shanghai 2>/dev/null || true
    # 部署更现代的 chrony 代替 ntp
    apt-get install -yq chrony > /dev/null 2>&1 || true
    systemctl enable --now chrony >/dev/null 2>&1 || true
    success "时区已设为 Asia/Shanghai。"
}

# ----------------- 综合优化入口 -----------------
run_base_optimization() {
    global_netcheck
    setup_base
    setup_kernel
    setup_sysctl
    setup_limits
    setup_security # 由 security.sh 提供
    setup_memory
    setup_low_memory_optimization
    setup_logrotate
    setup_timezone
    
    # 写入完成标记
    save_project_config "BASE_OPTIMIZED" "true"
    info "🔥 基础系统级优化全部完成！"
}

# ----------------- 路由转发管理 -----------------
get_ip_forward_status() {
    if [[ "$(sysctl -n net.ipv4.ip_forward 2>/dev/null)" == "1" ]]; then
        echo -e "${GREEN}●${NC} ${DIM}已开启${NC}"
    else
        echo -e "${DIM}○ 已关闭${NC}"
    fi
}

# 切换 IP 转发状态
toggle_ip_forwarding() {
    if [[ "$(sysctl -n net.ipv4.ip_forward 2>/dev/null)" == "1" ]]; then
        info "关闭系统 IP 转发功能..."
        rm -f /etc/sysctl.d/99-debopti-forwarding.conf
        sysctl -w net.ipv4.ip_forward=0 >/dev/null || true
        success "已切换为纯建站模式 (Forward Off)。"
    else
        info "开启系统 IP 转发功能..."
        render_template "templates/system/99-debopti-forwarding.conf" "/etc/sysctl.d/99-debopti-forwarding.conf"
        sysctl --system > /dev/null 2>&1
        success "已切换为组网/代理模式 (Forward On)。"
    fi
    sleep 1
}
