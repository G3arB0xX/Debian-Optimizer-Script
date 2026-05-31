#!/bin/bash
# =========================================================
# Xray Core 自动化部署与多规则集管理模块
# =========================================================

# ----------------- 核心安装与升级 -----------------
install_xray() {
    info "正在从 XTLS 官方源部署/更新 Xray Core..."
    
    # 获取官方安装脚本，利用 fallback 机制保障国内成功率
    download_with_fallback "/tmp/xray-install.sh" "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh" || return 1
    
    # 执行官方安装命令
    bash /tmp/xray-install.sh install || { err "Xray 核心安装失败。"; return 1; }
    
    # --- 关闭开机自启服务 (符合 VIBEINSTRCT.md 规范与用户要求) ---
    info "正在禁用 Xray 默认开机自启服务..."
    systemctl disable xray >/dev/null 2>&1 || true
    
    # --- 备份并初始化官方原始规则集 (geodata) ---
    local asset_dir="/usr/local/share/xray"
    mkdir -p "$asset_dir"
    
    if [[ -f "$asset_dir/geosite.dat" && ! -L "$asset_dir/geosite.dat" ]]; then
        mv -f "$asset_dir/geosite.dat" "$asset_dir/geosite.dat.official"
    fi
    if [[ -f "$asset_dir/geoip.dat" && ! -L "$asset_dir/geoip.dat" ]]; then
        mv -f "$asset_dir/geoip.dat" "$asset_dir/geoip.dat.official"
    fi
    
    # 极强健壮性防灾：如果官方默认规则未就绪，启动自愈式拉取
    if [[ ! -f "$asset_dir/geosite.dat.official" || ! -f "$asset_dir/geoip.dat.official" ]]; then
        info "检测到官方规则集缺失，正在执行自愈下载..."
        download_xray_official_files || true
    fi
    
    # --- 规则集与定时任务自适应自愈逻辑 ---
    local current_ruleset=$(grep -E "^XRAY_RULESET=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "official")
    [[ -z "$current_ruleset" ]] && current_ruleset="official"
    
    if [[ "$current_ruleset" == "loyalsoldier" ]]; then
        # 用户之前选过 Loyalsoldier，若规则文件丢失则重新拉取
        if [[ ! -f "$asset_dir/geosite.dat.loyalsoldier" || ! -f "$asset_dir/geoip.dat.loyalsoldier" ]]; then
            download_xray_loyalsoldier_files || true
        fi
        ln -sf geosite.dat.loyalsoldier "$asset_dir/geosite.dat"
        ln -sf geoip.dat.loyalsoldier "$asset_dir/geoip.dat"
        # 保持定时更新任务处于启用状态
        setup_xray_cron_job
    else
        # 默认使用官方规则
        ln -sf geosite.dat.official "$asset_dir/geosite.dat"
        ln -sf geoip.dat.official "$asset_dir/geoip.dat"
        # 官方规则下默认不启用/清理 Loyalsoldier 定时更新
        cleanup_xray_geodata_cron
        save_project_config "XRAY_RULESET" "official"
    fi

    # --- 安全沙箱加固 (Systemd Override) ---
    render_template "templates/apps/xray/xray.service.override.conf" "-" | inject_service_override "xray"
    
    success "Xray Core 部署已就绪 (开机自启已禁用，当前使用官方默认规则集)。"
}

# ----------------- 深度卸载与残留清除 -----------------
uninstall_xray() {
    info "准备深度清理 Xray Core 及其生态残留..."
    
    # 优先调用官方卸载逻辑
    if [[ ! -f "/tmp/xray-install.sh" ]]; then
        download_with_fallback "/tmp/xray-install.sh" "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh" || return 1
    fi
    bash /tmp/xray-install.sh remove >/dev/null 2>&1
    
    # 停止服务并清理 Systemd 单元
    systemctl stop xray >/dev/null 2>&1
    systemctl disable xray >/dev/null 2>&1
    rm -rf /etc/systemd/system/xray*
    systemctl daemon-reload
    
    # 清理第三方规则与自动更新定时任务
    cleanup_xray_geodata_cron
    
    # 暴力扫荡所有可能残留的二进制、规则及配置目录，确保环境原子化还原
    rm -rf /usr/bin/xray /usr/local/bin/xray /usr/local/etc/xray /etc/xray /opt/xray /etc/systemd/system/xray.service.d
    rm -rf /usr/local/share/xray
    
    # 清除配置文件状态标志
    save_project_config "XRAY_RULESET" ""
    
    success "Xray 及其规则集、定时任务已彻底从系统中移除。"
}

# ----------------- 规则集拉取与防灾备份 -----------------

# 拉取 Loyalsoldier 第三方规则集 (支持中国大陆网络加速)
download_xray_loyalsoldier_files() {
    local asset_dir="/usr/local/share/xray"
    mkdir -p "$asset_dir"
    
    # 极其防御性的环境变量加载与兜底
    local is_cn="${IS_CN_REGION:-}"
    if [[ -z "$is_cn" && -f "$CONFIG_FILE" ]]; then
        is_cn=$(grep -E "^IS_CN_REGION=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "false")
    fi
    [[ -z "$is_cn" ]] && is_cn="false"
    
    local geosite_url="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat"
    local geoip_url="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat"
    
    if [[ "$is_cn" == "true" ]]; then
        geosite_url="https://ghfast.top/${geosite_url}"
        geoip_url="https://ghfast.top/${geoip_url}"
    fi
    
    info "正在拉取 Loyalsoldier 增强版路由规则 (geosite.dat)..."
    if ! download_with_fallback "$asset_dir/geosite.dat.loyalsoldier.new" "$geosite_url"; then
        if [[ "$is_cn" != "true" ]]; then
            warn "GitHub 直连下载 geosite.dat 失败，尝试通过镜像加速源恢复..."
            download_with_fallback "$asset_dir/geosite.dat.loyalsoldier.new" "https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat" || return 1
        else
            return 1
        fi
    fi
    
    local g_size=$(stat -c%s "$asset_dir/geosite.dat.loyalsoldier.new" 2>/dev/null || echo 0)
    if [[ $g_size -gt 102400 ]]; then
        mv -f "$asset_dir/geosite.dat.loyalsoldier.new" "$asset_dir/geosite.dat.loyalsoldier"
    else
        rm -f "$asset_dir/geosite.dat.loyalsoldier.new"
        err "Loyalsoldier geosite.dat 校验失败 (文件体积异常)。"
        return 1
    fi
    
    info "正在拉取 Loyalsoldier 增强版地理IP规则 (geoip.dat)..."
    if ! download_with_fallback "$asset_dir/geoip.dat.loyalsoldier.new" "$geoip_url"; then
        if [[ "$is_cn" != "true" ]]; then
            warn "GitHub 直连下载 geoip.dat 失败，尝试通过镜像加速源恢复..."
            download_with_fallback "$asset_dir/geoip.dat.loyalsoldier.new" "https://ghfast.top/https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat" || return 1
        else
            return 1
        fi
    fi
    
    local ip_size=$(stat -c%s "$asset_dir/geoip.dat.loyalsoldier.new" 2>/dev/null || echo 0)
    if [[ $ip_size -gt 102400 ]]; then
        mv -f "$asset_dir/geoip.dat.loyalsoldier.new" "$asset_dir/geoip.dat.loyalsoldier"
    else
        rm -f "$asset_dir/geoip.dat.loyalsoldier.new"
        err "Loyalsoldier geoip.dat 校验失败 (文件体积异常)。"
        return 1
    fi
    
    success "Loyalsoldier 第三方规则集缓存同步成功。"
    return 0
}

# 恢复官方默认规则集 (防损自愈备份)
download_xray_official_files() {
    local asset_dir="/usr/local/share/xray"
    mkdir -p "$asset_dir"
    
    # 极其防御性的环境变量加载与兜底
    local is_cn="${IS_CN_REGION:-}"
    if [[ -z "$is_cn" && -f "$CONFIG_FILE" ]]; then
        is_cn=$(grep -E "^IS_CN_REGION=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "false")
    fi
    [[ -z "$is_cn" ]] && is_cn="false"
    
    local geosite_url="https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat"
    local geoip_url="https://github.com/v2fly/geoip/releases/latest/download/geoip.dat"
    
    if [[ "$is_cn" == "true" ]]; then
        geosite_url="https://ghfast.top/${geosite_url}"
        geoip_url="https://ghfast.top/${geoip_url}"
    fi
    
    info "正在从上游恢复官方路由规则 (geosite.dat.official)..."
    if ! download_with_fallback "$asset_dir/geosite.dat.official.new" "$geosite_url"; then
        if [[ "$is_cn" != "true" ]]; then
            warn "GitHub 直连下载官方 dlc.dat 失败，尝试通过镜像加速源恢复..."
            download_with_fallback "$asset_dir/geosite.dat.official.new" "https://ghfast.top/https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat" || return 1
        else
            return 1
        fi
    fi
    
    local g_size=$(stat -c%s "$asset_dir/geosite.dat.official.new" 2>/dev/null || echo 0)
    if [[ $g_size -gt 102400 ]]; then
        mv -f "$asset_dir/geosite.dat.official.new" "$asset_dir/geosite.dat.official"
    else
        rm -f "$asset_dir/geosite.dat.official.new"
        err "官方 geosite.dat 校验失败。"
        return 1
    fi
    
    info "正在从上游恢复官方地理IP规则 (geoip.dat.official)..."
    if ! download_with_fallback "$asset_dir/geoip.dat.official.new" "$geoip_url"; then
        if [[ "$is_cn" != "true" ]]; then
            warn "GitHub 直连下载官方 geoip.dat 失败，尝试通过镜像加速源恢复..."
            download_with_fallback "$asset_dir/geoip.dat.official.new" "https://ghfast.top/https://github.com/v2fly/geoip/releases/latest/download/geoip.dat" || return 1
        else
            return 1
        fi
    fi
    
    local ip_size=$(stat -c%s "$asset_dir/geoip.dat.official.new" 2>/dev/null || echo 0)
    if [[ $ip_size -gt 102400 ]]; then
        mv -f "$asset_dir/geoip.dat.official.new" "$asset_dir/geoip.dat.official"
    else
        rm -f "$asset_dir/geoip.dat.official.new"
        err "官方 geoip.dat 校验失败。"
        return 1
    fi
    
    success "官方默认规则集自愈拉取成功。"
    return 0
}

# ----------------- 规则集一键切换 (核心算法) -----------------
toggle_xray_ruleset() {
    local asset_dir="/usr/local/share/xray"
    local active_ruleset=$(grep -E "^XRAY_RULESET=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "official")
    [[ -z "$active_ruleset" ]] && active_ruleset="official"
    
    if [[ "$active_ruleset" == "official" ]]; then
        info "正在为您配置 Loyalsoldier 第三方规则集..."
        # 1. 确保第三方规则集本地有缓存
        if [[ ! -f "$asset_dir/geosite.dat.loyalsoldier" || ! -f "$asset_dir/geoip.dat.loyalsoldier" ]]; then
            download_xray_loyalsoldier_files || return 1
        fi
        
        # 2. 精准原子修改软链接指向
        ln -sf geosite.dat.loyalsoldier "$asset_dir/geosite.dat"
        ln -sf geoip.dat.loyalsoldier "$asset_dir/geoip.dat"
        
        # 3. 智能联动：切换到 Loyalsoldier 后，自动激活自动更新定时任务
        setup_xray_cron_job
        
        # 4. 写入全局配置文件
        save_project_config "XRAY_RULESET" "loyalsoldier"
        success "已成功切换为：Loyalsoldier 增强规则集 (已自动配置 Cron 定时更新任务)。"
    else
        info "正在为您恢复官方默认规则集..."
        # 1. 确保官方规则集本地有缓存
        if [[ ! -f "$asset_dir/geosite.dat.official" || ! -f "$asset_dir/geoip.dat.official" ]]; then
            download_xray_official_files || return 1
        fi
        
        # 2. 精准原子修改软链接指向
        ln -sf geosite.dat.official "$asset_dir/geosite.dat"
        ln -sf geoip.dat.official "$asset_dir/geoip.dat"
        
        # 3. 智能联动：切换回官方规则集后，自动卸载 Loyalsoldier 自动更新任务
        cleanup_xray_geodata_cron
        
        # 4. 写入全局配置文件
        save_project_config "XRAY_RULESET" "official"
        success "已成功切换为：官方默认规则集 (已自动清理 Cron 定时更新任务)。"
    fi
    
    # 5. Xray 服务热重载 (如果正在运行)
    if systemctl is-active --quiet xray; then
        info "检测到 Xray Core 服务处于活动状态，正在热重启服务以加载新规则..."
        systemctl restart xray >/dev/null 2>&1 || true
        success "Xray Core 已完成热重载。"
    fi
}

# ----------------- 自动更新定时任务 (Cron) 控制 -----------------

# 部署或更新自动更新脚本及 Crontab 任务
setup_xray_cron_job() {
    local cron_script="/usr/local/bin/xray-rule-update.sh"
    
    # 极其防御性的环境变量加载与兜底
    local is_cn="${IS_CN_REGION:-}"
    if [[ -z "$is_cn" && -f "$CONFIG_FILE" ]]; then
        is_cn=$(grep -E "^IS_CN_REGION=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "false")
    fi
    [[ -z "$is_cn" ]] && is_cn="false"

    # 注入环境参数并生成安全高可用的更新脚本
    render_template "templates/apps/xray/xray-rule-update.sh" "$cron_script" "IS_CN=$is_cn"
    chmod +x "$cron_script"
    
    # 极其强健的 crontab 幂等性注入逻辑，兼容没有初始 crontab 的环境
    local current_cron=""
    current_cron=$(crontab -l 2>/dev/null || echo "")
    if ! echo "$current_cron" | grep -q "$cron_script"; then
        { echo "$current_cron"; echo "30 3 * * 1 $cron_script >/dev/null 2>&1"; } | crontab - || true
    fi
}

# 彻底清理 Crontab 任务及脚本
cleanup_xray_geodata_cron() {
    local cron_script="/usr/local/bin/xray-rule-update.sh"
    rm -f "$cron_script"
    
    local current_cron=""
    current_cron=$(crontab -l 2>/dev/null || echo "")
    if echo "$current_cron" | grep -q "$cron_script"; then
        echo "$current_cron" | grep -v "$cron_script" | crontab - || true
    fi
}

# 获取当前 Cron 定时更新任务状态
get_xray_cron_status() {
    local cron_script="/usr/local/bin/xray-rule-update.sh"
    local current_cron=""
    current_cron=$(crontab -l 2>/dev/null || echo "")
    if echo "$current_cron" | grep -q "$cron_script"; then
        echo -e "${GREEN}●${NC} ${DIM}已启用${NC}"
    else
        echo -e "${DIM}○ 已禁用${NC}"
    fi
}

# 开关 Cron 定时任务切换函数
toggle_xray_cron() {
    local cron_script="/usr/local/bin/xray-rule-update.sh"
    local current_cron=""
    current_cron=$(crontab -l 2>/dev/null || echo "")
    if echo "$current_cron" | grep -q "$cron_script"; then
        cleanup_xray_geodata_cron
        success "已成功关闭 Xray 规则自动更新定时任务。"
    else
        setup_xray_cron_job
        success "已成功开启 Xray 规则自动更新定时任务 (每周一凌晨 3:30 自动运行)。"
    fi
}

# ----------------- TUI 控制台子菜单 (Xray 专属) -----------------
handle_xray_submenu() {
    while true; do
        # 实时从配置中心加载规则集状态
        local active_ruleset=$(grep -E "^XRAY_RULESET=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "official")
        [[ -z "$active_ruleset" ]] && active_ruleset="official"
        
        local ruleset_tag=""
        if [[ "$active_ruleset" == "loyalsoldier" ]]; then
            ruleset_tag="${YELLOW}[Loyalsoldier]${NC}"
        else
            ruleset_tag="${GREEN}[官方默认]${NC}"
        fi

        ui_draw_header "Xray Core 节点管理" "Main > Xray | 规则: $ruleset_tag"
        
        ui_draw_item "1" "✨ 安装 / 更新 Xray Core" "$(get_status xray)"
        ui_draw_item "2" "🗑️ 卸载并清理 Xray Core"
        
        # 选项 3 动态识别当前状态并生成对立操作
        if [[ "$active_ruleset" == "loyalsoldier" ]]; then
            ui_draw_item "3" "🔄 切换至 官方默认规则集"
        else
            ui_draw_item "3" "🔄 切换至 Loyalsoldier 增强规则集"
        fi
        
        ui_draw_item "4" "⏰ 规则自动更新任务" "$(get_xray_cron_status)"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 请输入选择: " sub_choice
        case $sub_choice in
            1) 
                install_xray
                pause
                ;;
            2) 
                read -p "确定要深度卸载并移除 Xray 吗？[y/N]: " confirm
                if [[ "$confirm" =~ ^[Yy]$ ]]; then
                    uninstall_xray
                    pause
                    break
                fi
                ;;
            3)
                if [[ "$(get_status xray)" == *"未部署"* ]]; then
                    warn "检测到系统未部署 Xray Core，请先执行选项 1 安装。"
                    pause
                else
                    toggle_xray_ruleset
                    pause
                fi
                ;;
            4)
                if [[ "$(get_status xray)" == *"未部署"* ]]; then
                    warn "检测到系统未部署 Xray Core，请先执行选项 1 安装。"
                    pause
                else
                    toggle_xray_cron
                    pause
                fi
                ;;
            0) break;;
            *) warn "无效输入。请重新选择！";;
        esac
    done
}
