#!/bin/bash
# =========================================================
# FreshIP IP 养护 自动化部署与管理
# =========================================================
# 准则: VIBEINSTRCT.md
# 架构: 双栈并发、原子更新、Systemd 沙盒化
# =========================================================

# ----------------- 内部工具函数 (私有) -----------------

# [内部] 交互式区域选择逻辑
_freship_select_region() {
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    info "正在同步全球节点地理索引..."
    curl -sL "${repo_raw}/data/map.json" -o "/tmp/freship_map.json"
    
    if [[ ! -s "/tmp/freship_map.json" ]]; then
        err "网络环境波动，无法获取节点地图，请检查 DNS 或网络连接。"
        return 1
    fi

    # 1. 国家选择
    echo -e "\n📍 请选择目标养护国家/地区 Country/Region："
    mapfile -t c_ids   < <(jq -r '.continents[].countries[].id'           /tmp/freship_map.json)
    mapfile -t c_names < <(jq -r '.continents[].countries[].name'         /tmp/freship_map.json)
    mapfile -t c_kws   < <(jq -r '.continents[].countries[].keyword_file' /tmp/freship_map.json)

    for i in "${!c_ids[@]}"; do
        printf "  %2d) %s\n" "$(( i+1 ))" "${c_names[$i]}"
    done
    read -rp "请输入序号 (默认 1): " c_sel
    c_sel=$(( ${c_sel:-1} - 1 ))
    [[ "$c_sel" -lt 0 || "$c_sel" -ge "${#c_ids[@]}" ]] && c_sel=0
    country_id="${c_ids[$c_sel]}"
    kw_filename="${c_kws[$c_sel]}"

    # 2. 州/省选择
    mapfile -t s_ids   < <(jq -r --arg c "$country_id" '.continents[].countries[]|select(.id==$c)|.states[].id' /tmp/freship_map.json)
    mapfile -t s_names < <(jq -r --arg c "$country_id" '.continents[].countries[]|select(.id==$c)|.states[].name' /tmp/freship_map.json)
    local state_id=""
    if [[ "${#s_ids[@]}" -eq 1 ]]; then
        state_id="${s_ids[0]}"
    else
        echo -e "\n📍 请选择具体州/省 State/Province："
        for i in "${!s_ids[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${s_names[$i]}"; done
        read -rp "请输入序号 (默认 1): " s_sel
        s_sel=$(( ${s_sel:-1} - 1 ))
        [[ "$s_sel" -lt 0 || "$s_sel" -ge "${#s_ids[@]}" ]] && s_sel=0
        state_id="${s_ids[$s_sel]}"
    fi

    # 3. 城市选择
    mapfile -t ci_ids   < <(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[].id' /tmp/freship_map.json)
    mapfile -t ci_names < <(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[].name' /tmp/freship_map.json)
    if [[ "${#ci_ids[@]}" -eq 1 ]]; then
        city_id="${ci_ids[0]}"; city_name="${ci_names[0]}"
    else
        echo -e "\n📍 请选择具体目标城市 City："
        for i in "${!ci_ids[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${city_names[$i]}"; done
        read -rp "请输入序号 (默认 1): " ci_sel
        ci_sel=$(( ${ci_sel:-1} - 1 ))
        [[ "$ci_sel" -lt 0 || "$ci_sel" -ge "${#ci_ids[@]}" ]] && ci_sel=0
        city_id="${city_ids[$ci_sel]}"; city_name="${ci_names[$ci_sel]}"
    fi
    rm -f /tmp/freship_map.json
    return 0
}

# [内部] 交互式 IP 与运行模式选择
_freship_select_mode() {
    info "正在探测本机公网出口 IP 状态..."
    local detect_v4=$(curl -4 -s -m 5 api.ip.sb/ip 2>/dev/null || curl -4 -s -m 5 ifconfig.me 2>/dev/null || echo "")
    local detect_v6=$(curl -6 -s -m 5 api.ip.sb/ip 2>/dev/null || curl -6 -s -m 5 icanhazip.com 2>/dev/null || echo "")
    detect_v4=$(echo "$detect_v4" | tr -d '[:space:]')
    detect_v6=$(echo "$detect_v6" | tr -d '[:space:]')
    
    bind_v4=""; bind_v6=""; work_mode=""

    if [[ -n "$detect_v4" && -n "$detect_v6" ]]; then
        echo -e "\n检测到双栈 Dual-Stack IP 环境，请选择养护模式："
        echo "  1) 仅 IPv4 养护"
        echo "  2) 仅 IPv6 养护"
        echo "  3) 双栈独立并发养护 (推荐)"
        read -rp "请输入序号 (默认 3): " mode_sel
        case "${mode_sel:-3}" in
            1) work_mode="ipv4_only"; bind_v4="$detect_v4" ;;
            2) work_mode="ipv6_only"; bind_v6="$detect_v6" ;;
            *) work_mode="dual_stack"; bind_v4="$detect_v4"; bind_v6="$detect_v6" ;;
        esac
    elif [[ -n "$detect_v4" ]]; then
        info "检测到单 IPv4 环境，已自动选择该模式。"
        work_mode="ipv4_only"; bind_v4="$detect_v4"
    elif [[ -n "$detect_v6" ]]; then
        info "检测到单 IPv6 环境，已自动选择该模式。"
        work_mode="ipv6_only"; bind_v6="$detect_v6"
    else
        warn "未能自动探测 IP，可能由于防火墙拦截或无公网 IP，请手动输入："
        read -rp "IPv4 地址 (留空则跳过 v4): " bind_v4
        read -rp "IPv6 地址 (留空则跳过 v6): " bind_v6
        [[ -n "$bind_v4" && -n "$bind_v6" ]] && work_mode="dual_stack"
        [[ -n "$bind_v4" && -z "$bind_v6" ]] && work_mode="ipv4_only"
        [[ -z "$bind_v4" && -n "$bind_v6" ]] && work_mode="ipv6_only"
    fi
    return 0
}

# ----------------- 核心业务逻辑 -----------------

# 1. 完整安装流程
install_freship() {
    info "正在部署 FreshIP..."

    # 1. 环境与依赖预检
    safe_apt_install curl jq unzip file coreutils less bc || return 1
    create_system_user "freship"

    # 2. 目录架构初始化与幂等清理
    local opt_dir="/opt/freship/opt"
    local conf_dir="/etc/freship"
    local log_dir="/var/log/freship"
    local config_file="${conf_dir}/freship.conf"
    local src_dir="${SCRIPT_DIR}/ip-sentinel-lite-v2"

    # 清理旧数据以防冲突
    [[ -d "$opt_dir/data/regions" ]] && rm -rf "$opt_dir/data/regions/*"

    mkdir -p "$opt_dir" "$conf_dir" "$log_dir"
    mkdir -p "${opt_dir}/bin" "${opt_dir}/core" "${opt_dir}/data/keywords" "${opt_dir}/data/regions"
    chown -R freship:freship "$log_dir"

    # 3. 部署 TLS 伪装引擎 (curl-impersonate)
    local arch=$(uname -m)
    local pkg_arch=""
    case "$arch" in
        x86_64)  pkg_arch="x86_64" ;;
        aarch64) pkg_arch="aarch64" ;;
    esac

    if [[ -n "$pkg_arch" ]]; then
        info "正在部署 TLS 伪装引擎 (curl-impersonate v0.6.1)..."
        local dl_url="https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.linux-${pkg_arch}.tar.gz"
        local tmp_tar="/tmp/freship_curl.tar.gz"
        if download_with_fallback "$tmp_tar" "$dl_url"; then
            tar -xzf "$tmp_tar" -C "${opt_dir}/bin" --wildcards 'curl_chrome*' 2>/dev/null || tar -xzf "$tmp_tar" -C "${opt_dir}/bin" 2>/dev/null
            rm -f "$tmp_tar"
            chmod +x "${opt_dir}/bin/curl_chrome"* 2>/dev/null
            success "TLS 伪装引擎已就绪。"
        fi
    fi

    # 4. 交互式业务配置
    local country_id city_name city_id kw_filename
    _freship_select_region || return 1

    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1

    read -rp "Telegram Bot Token 可选，跳过请直按回车: " tg_token
    local chat_id=""
    [[ -n "$tg_token" ]] && read -rp "Chat ID (通过 @userinfobot 获取): " chat_id

    # 5. 资产同步与核心脚本去痕迹化 Patch
    info "同步全球养护资产并执行核心逻辑 Patch..."
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    
    # 首次数据拉取 (原子操作模式由 updater 负责后续维护)
    local region_target="${opt_dir}/data/regions/${city_id//\//.}.json"
    curl -sL "${repo_raw}/data/regions/${country_id}/${city_id}.json" -o "$region_target" 2>/dev/null || \
    curl -sL "${repo_raw}/data/regions/${country_id}/$(basename "$city_id").json" -o "$region_target" 2>/dev/null
    curl -sL "${repo_raw}/data/keywords/${kw_filename}" -o "${opt_dir}/data/keywords/${kw_filename}"
    curl -sL "${repo_raw}/data/user_agents.txt" -o "${opt_dir}/data/user_agents.txt"

    # 复制并深度 Patch 核心脚本
    local core_script="${opt_dir}/core/freship_core.sh"
    cp "${src_dir}/sentinel.sh" "$core_script"
    
    # 执行品牌重置与路径校准
    sed -i "s|IP-Sentinel|FreshIP|g; s|ip_sentinel|freship|g; s|sentinel|freship|g; s|Sentinel|FreshIP|g" "$core_script"
    sed -i "s|INSTALL_DIR=\"/opt/ip_sentinel\"|INSTALL_DIR=\"${opt_dir}\"|g" "$core_script"
    sed -i "s|CONFIG_FILE=\"\${INSTALL_DIR}/config.conf\"|CONFIG_FILE=\"${config_file}\"|g" "$core_script"
    sed -i "s|LOG_FILE=\"\${INSTALL_DIR}/logs/sentinel.log\"|LOG_FILE=\"${log_dir}/freship.log\"|g" "$core_script"
    sed -i "s|LOCK_FILE=\"/tmp/ip_sentinel.lock\"|LOCK_FILE=\"/tmp/freship_\\\${INSTANCE_MODE:-global}.lock\"|g" "$core_script"

    # 注入生产级防御代码 (依赖检查与双栈逻辑)
    sed -i "2i # --- Defensive programming & Dependency Check ---\\
if ! command -v jq >/dev/null 2>&1; then echo 'Error: jq not found' && exit 1; fi\\
trap 'rm -f /tmp/freship_\\\${INSTANCE_MODE:-global}.lock' EXIT\\
INSTANCE_MODE=\$1" "$core_script"

    sed -i "/source \"\$CONFIG_FILE\"/a \\
# --- Dual-Stack Override Logic --- \\
if [[ \"\$INSTANCE_MODE\" == \"v4\" ]]; then \\
    BIND_IP=\"\$BIND_IPV4\" \\
    IP_PREF=\"4\" \\
elif [[ \"\$INSTANCE_MODE\" == \"v6\" ]]; then \\
    BIND_IP=\"\$BIND_IPV6\" \\
    IP_PREF=\"6\" \\
fi" "$core_script"

    # 部署高可靠更新器 (Atomic Updater)
    _freship_deploy_updater "$config_file" "$opt_dir" "$log_dir"

    chmod +x "${opt_dir}/core/"*.sh

    # 6. 持久化配置文件
    cat > "$config_file" << EOF
# =========================================================
# FreshIP (IP 养护) 配置文件
# 生成时间: $(date '+%Y-%m-%d %H:%M:%S')
# =========================================================

# [区域定义]
REGION_CODE="${country_id}"
REGION_NAME="${city_name}"

# [网络定义]
BIND_IPV4="${bind_v4}"
BIND_IPV6="${bind_v6}"
WORK_MODE="${work_mode}"

# [模块策略]
ENABLE_GOOGLE="true"
ENABLE_TRUST="true"

# [通知渠道]
TG_TOKEN="${tg_token}"
CHAT_ID="${chat_id}"

# [内核路径]
INSTALL_DIR="${opt_dir}"
LOG_FILE="${log_dir}/freship.log"
EOF
    chown -R freship:freship "$conf_dir" "$log_dir" "$opt_dir"
    chmod 600 "$config_file"

    # 7. 部署 Systemd 服务单元
    deploy_freship_systemd "$work_mode"

    success "FreshIP 部署成功。"
}

# 2. [内部] 原子更新器生成器
_freship_deploy_updater() {
    local config=$1 opt=$2 log=$3
    cat > "${opt}/core/freship_updater.sh" << EOF
#!/bin/bash
# =========================================================
# FreshIP 数据资产原子更新器
# =========================================================
[[ ! -f "${config}" ]] && exit 1
source "${config}"
LOG_FILE="${log}/freship.log"
REPO_RAW="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
UA_TIMESTAMP="${opt}/core/.ua_last_update"

log() { printf "[%s UTC] [Updater] [%s] %s\n" "\$(date -u '+%Y-%m-%d %H:%M:%S')" "\${REGION_CODE}" "\$1" >> "\$LOG_FILE"; }

log "INFO: 启动资产完整性校验更新..."
TMP_DIR="/tmp/freship_ota_\$\$"
mkdir -p "\$TMP_DIR"
CURL_OPTS="curl -\${IP_PREF:-4} -sL --max-time 60 --connect-timeout 10"

# 影子下载与结构嗅探
LOCAL_JSON=\$(find "${opt}/data/regions" -name "*.json" 2>/dev/null | head -n 1)
REL_PATH="\${LOCAL_JSON#${opt}/}"
DOWNLOAD_OK=1

if [[ -n "\$REL_PATH" ]]; then
    \$CURL_OPTS "\${REPO_RAW}/\$REL_PATH" -o "\$TMP_DIR/region.json" || DOWNLOAD_OK=0
fi
\$CURL_OPTS "\${REPO_RAW}/data/keywords/kw_\${REGION_CODE}.txt" -o "\$TMP_DIR/kw.txt" || DOWNLOAD_OK=0

# 原子性逻辑校验
if [[ \$DOWNLOAD_OK -eq 1 ]] && [[ -s "\$TMP_DIR/kw.txt" ]] && jq . "\$TMP_DIR/region.json" >/dev/null 2>&1; then
    mv "\$TMP_DIR/kw.txt" "${opt}/data/keywords/kw_\${REGION_CODE}.txt"
    mv "\$TMP_DIR/region.json" "\$LOCAL_JSON"
    log "INFO: 数据资产原子替换成功。"
else
    log "ERROR: 校验未通过 (网络超时或远端结构变更)，回滚本地环境。"
fi
rm -rf "\$TMP_DIR"
EOF
}

# 3. 交互式重配置流程
reconfigure_freship() {
    local config_file="/etc/freship/freship.conf"
    [[ ! -f "$config_file" ]] && { err "未找到 FreshIP 配置，请先执行安装。"; return 1; }

    info "进入 FreshIP 配置重置流程..."
    
    local country_id city_name city_id kw_filename
    _freship_select_region || return 1

    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1

    source "$config_file"
    echo -e "\n当前推送 Token: ${TG_TOKEN:-未配置}"
    read -rp "是否重新配置 Telegram 推送？[y/N]: " change_tg
    if [[ "$change_tg" =~ ^[Yy]$ ]]; then
        read -rp "新 Bot Token (留空则禁用): " tg_token
        chat_id=""
        [[ -n "$tg_token" ]] && read -rp "接收者 ID: " chat_id
    else
        tg_token="$TG_TOKEN"; chat_id="$CHAT_ID"
    fi

    # 持久化新配置
    cat > "$config_file" << EOF
# =========================================================
# FreshIP (IP 养护) 配置文件 (已更新)
# =========================================================
REGION_CODE="${country_id}"
REGION_NAME="${city_name}"
BIND_IPV4="${bind_v4}"
BIND_IPV6="${bind_v6}"
WORK_MODE="${work_mode}"
ENABLE_GOOGLE="true"
ENABLE_TRUST="true"
TG_TOKEN="${tg_token}"
CHAT_ID="${chat_id}"
INSTALL_DIR="/opt/freship/opt"
LOG_FILE="/var/log/freship/freship.log"
EOF

    info "配置重置成功。正在执行服务热重载与数据强制同步..."
    # 联动操作
    /bin/bash /opt/freship/opt/core/freship_updater.sh
    uninstall_freship >/dev/null 2>&1 || true
    deploy_freship_systemd "$work_mode"

    success "配置已生效。"
}

# 4. Systemd 生产级服务编排
deploy_freship_systemd() {
    local mode=$1
    info "正在编排 Systemd 服务 (模式: $mode)..."
    
    # 核心养护任务 (Template)
    cat > /etc/systemd/system/freship-core@.service << EOF
[Unit]
Description=FreshIP Maintenance Engine (%i)
After=network.target

[Service]
Type=oneshot
User=freship
Group=freship
ExecStart=/bin/bash /opt/freship/opt/core/freship_core.sh %i
# 标准输出交由脚本内部 log 负责，此处仅记录系统级异常
StandardOutput=journal
StandardError=journal

# --- Production Hardening ---
ProtectSystem=full
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectHostname=true
NoNewPrivileges=true
RestrictRealtime=true
ReadOnlyPaths=/opt/freship/opt
ReadWritePaths=/var/log/freship /etc/freship
# ---------------------------
EOF

    cat > /etc/systemd/system/freship-core@.timer << EOF
[Unit]
Description=FreshIP Maintenance Timer (%i)

[Timer]
OnBootSec=5min
OnUnitActiveSec=20min
RandomizedDelaySec=300
Unit=freship-core@%i.service

[Install]
WantedBy=timers.target
EOF

    # 数据更新任务
    cat > /etc/systemd/system/freship-updater.service << EOF
[Unit]
Description=FreshIP Data OTA Updater
After=network.target

[Service]
Type=oneshot
User=freship
Group=freship
ExecStart=/bin/bash /opt/freship/opt/core/freship_updater.sh
ReadWritePaths=/var/log/freship /etc/freship /opt/freship/opt/data
EOF

    cat > /etc/systemd/system/freship-updater.timer << EOF
[Unit]
Description=FreshIP Daily Update Timer

[Timer]
OnCalendar=*-*-* 03:00:00
RandomizedDelaySec=3600
Persistent=true
Unit=freship-updater.service

[Install]
WantedBy=timers.target
EOF

    systemctl daemon-reload
    case "$mode" in
        ipv4_only) systemctl enable --now freship-core@v4.timer ;;
        ipv6_only) systemctl enable --now freship-core@v6.timer ;;
        dual_stack) systemctl enable --now freship-core@v4.timer; systemctl enable --now freship-core@v6.timer ;;
    esac
    systemctl enable --now freship-updater.timer
}

# 5. 深度清理逻辑
uninstall_freship() {
    success "正在卸载 FreshIP..."
    systemctl disable --now freship-core@v4.timer freship-core@v6.timer freship-updater.timer >/dev/null 2>&1
    rm -f /etc/systemd/system/freship-core@* /etc/systemd/system/freship-updater.*
    systemctl daemon-reload
    rm -rf /opt/freship /etc/freship /var/log/freship
    id -u freship >/dev/null 2>&1 && userdel freship
    success "清理完成。"
}

# 6. 管理面板
manage_freship() {
    while true; do
        clear
        local v4_status=$(systemctl is-active freship-core@v4.timer --quiet && echo -e "${GREEN}[活跃]${NC}" || echo -e "${RED}[下线]${NC}")
        local v6_status=$(systemctl is-active freship-core@v6.timer --quiet && echo -e "${GREEN}[活跃]${NC}" || echo -e "${RED}[下线]${NC}")
        
        echo -e "🚀 [ FreshIP 管理 ]"
        echo "----------------------------------------------"
        echo -e " 状态: IPv4 $v4_status | IPv6 $v6_status"
        echo "----------------------------------------------"
        echo " 1. 启动任务"
        echo " 2. 停止任务"
        echo " 3. 重置配置 Region/IP/Mode"
        echo " 4. 查看日志"
        echo " 5. 立即同步资产"
        echo " 6. 卸载模块"
        echo "----------------------------------------------"
        echo " 0. 返回上级菜单"
        read -p "指令选择: " opt
        case $opt in
            1) 
                source /etc/freship/freship.conf 2>/dev/null
                [[ "$WORK_MODE" == "ipv4_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
                [[ "$WORK_MODE" == "ipv6_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
                systemctl enable --now freship-updater.timer
                success "服务集群已重新上线。"; pause;;
            2) systemctl disable --now freship-core@v4.timer freship-core@v6.timer freship-updater.timer; info "🛑 全球任务已挂起。"; pause;;
            3) reconfigure_freship; pause;;
            4) less +G /var/log/freship/freship.log;;
            5) info "强制触发影子同步..."; systemctl start freship-updater.service; success "指令已执行。"; pause;;
            6) read -p "确认销毁 FreshIP 生产环境？[y/N]: " confirm; [[ "$confirm" =~ ^[Yy]$ ]] && { uninstall_freship; pause; return; };;
            0) break;;
            *) warn "无效输入。";;
        esac
    done
}
