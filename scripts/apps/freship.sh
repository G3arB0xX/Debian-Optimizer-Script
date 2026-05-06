#!/bin/bash
# =========================================================
# FreshIP IP 养护 自动化部署与管理
# =========================================================
# 准则: VIBEINSTRCT.md
# 架构: 双栈并发、本地化引擎、Systemd 沙盒化
# =========================================================

# ----------------- 内部工具函数 (私有) -----------------

# [内部] 交互式区域选择逻辑
_freship_select_region() {
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    info "正在同步全球节点地理索引..."
    
    if ! download_with_fallback "/tmp/freship_map.json" "${repo_raw}/data/map.json"; then
        err "网络环境波动，无法获取节点地图，请检查 DNS 或网络连接。"
        return 1
    fi

    # 1. 国家选择
    mapfile -t c_ids   < <(jq -r '.continents[].countries[].id'           /tmp/freship_map.json)
    mapfile -t c_names < <(jq -r '.continents[].countries[].name'         /tmp/freship_map.json)
    mapfile -t c_kws   < <(jq -r '.continents[].countries[].keyword_file' /tmp/freship_map.json)

    if [[ ${#c_ids[@]} -eq 0 ]]; then
        err "获取国家列表失败，可能是 map.json 解析错误。"
        return 1
    fi

    echo -e "\n📍 请选择目标养护国家/地区 Country/Region："
    for i in "${!c_ids[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${c_names[$i]}"; done
    read -rp "请输入序号 (默认 1): " c_sel
    c_sel=$(( ${c_sel:-1} - 1 ))
    [[ "$c_sel" -lt 0 || "$c_sel" -ge "${#c_ids[@]}" ]] && c_sel=0
    country_id="${c_ids[$c_sel]}"
    kw_filename="${c_kws[$c_sel]}"

    # 2. 州/省选择 (简化逻辑：优先选 Default)
    state_id=$(jq -r --arg c "$country_id" '.continents[].countries[]|select(.id==$c)|.states[0].id' /tmp/freship_map.json 2>/dev/null)
    
    # 3. 城市选择 (简化逻辑：优先选第一个城市)
    city_id=$(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[0].id' /tmp/freship_map.json 2>/dev/null)
    city_name=$(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[0].name' /tmp/freship_map.json 2>/dev/null)

    # 路径构建
    remote_path="${country_id}/${state_id}/${city_id}"
    
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
        warn "未能自动探测 IP，请手动输入："
        read -rp "IPv4 地址 (留空跳过): " bind_v4
        read -rp "IPv6 地址 (留空跳过): " bind_v6
        [[ -n "$bind_v4" && -n "$bind_v6" ]] && work_mode="dual_stack"
        [[ -n "$bind_v4" && -z "$bind_v6" ]] && work_mode="ipv4_only"
        [[ -z "$bind_v4" && -n "$bind_v6" ]] && work_mode="ipv6_only"
    fi
    return 0
}

# ----------------- 核心业务逻辑 -----------------

install_freship() {
    info "正在部署 FreshIP..."

    # 1. 环境准备
    safe_apt_install curl jq unzip file coreutils less bc || return 1
    create_system_user "freship"

    local opt_dir="/opt/freship"
    local conf_dir="/etc/freship"
    local log_dir="/var/log/freship"
    local config_file="${conf_dir}/freship.conf"

    # 如果已存在旧版，先进行清理以防冲突 (特别是 lite-v2 等旧路径)
    [[ -d "/opt/freship" ]] && rm -rf "/opt/freship"
    
    mkdir -p "$opt_dir/bin" "$opt_dir/core" "$opt_dir/data/keywords" "$opt_dir/data/regions" "$conf_dir" "$log_dir"
    chown -R freship:freship "$log_dir"

    # 2. 部署 TLS 伪装引擎
    local arch=$(uname -m)
    local pkg_arch="x86_64-linux-gnu"
    [[ "$arch" == "aarch64" ]] && pkg_arch="aarch64-linux-gnu"
    
    info "正在部署 TLS 伪装引擎..."
    local dl_url="https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.${pkg_arch}.tar.gz"
    local tmp_tar="/tmp/freship_curl.tar.gz"
    if download_with_fallback "$tmp_tar" "$dl_url"; then
        tar -xzf "$tmp_tar" -C "${opt_dir}/bin" 2>/dev/null
        rm -f "$tmp_tar"
        chmod +x "${opt_dir}/bin/"* 2>/dev/null
    fi

    # 3. 配置
    local country_id city_name city_id remote_path kw_filename
    _freship_select_region || return 1
    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1

    # 4. 资产拉取
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    download_with_fallback "${opt_dir}/data/regions/${city_id}.json" "${repo_raw}/data/regions/${remote_path}.json" || true
    download_with_fallback "${opt_dir}/data/keywords/${kw_filename}" "${repo_raw}/data/keywords/${kw_filename}" || true
    download_with_fallback "${opt_dir}/data/user_agents.txt" "${repo_raw}/data/user_agents.txt" || true

    # 5. 写入核心脚本 (本地生成)
    local core_script="${opt_dir}/core/freship_core.sh"
    cat > "$core_script" << 'EOF'
#!/bin/bash
set -e
CONFIG_FILE="/etc/freship/freship.conf"
[[ ! -f "$CONFIG_FILE" ]] && exit 1
source "$CONFIG_FILE"
INSTANCE_MODE=${1:-"global"}
LOG_FILE="${LOG_FILE:-/var/log/freship/freship.log}"
UA_FILE="${INSTALL_DIR}/data/user_agents.txt"
KW_FILE="${INSTALL_DIR}/data/keywords/kw_${REGION_CODE}.txt"

log() { printf "[%s] [%s] [%s] %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$REGION_CODE" "$INSTANCE_MODE" "$1" >> "$LOG_FILE"; }

LOCK_FILE="/tmp/freship_${INSTANCE_MODE}.lock"
echo $$ > "$LOCK_FILE"
trap 'rm -f "$LOCK_FILE"' EXIT

BIND_IP=""
[[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="$BIND_IPV4"
[[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="$BIND_IPV6"
[[ -z "$BIND_IP" ]] && exit 0

keyword=$( [ -f "$KW_FILE" ] && shuf -n 1 "$KW_FILE" || echo "Debian Linux" )
ua=$( [ -f "$UA_FILE" ] && shuf -n 1 "$UA_FILE" || echo "Mozilla/5.0" )

CURL_BIN="curl"
[ -f "${INSTALL_DIR}/bin/curl_chrome116" ] && CURL_BIN="${INSTALL_DIR}/bin/curl_chrome116"

log "INFO: 启动养护任务 ($BIND_IP) -> $keyword"
STATUS=$( $CURL_BIN -s -o /dev/null -w "%{http_code}" --interface "$BIND_IP" -A "$ua" "https://www.google.com/search?q=${keyword// /+}" )
log "RESULT: HTTP $STATUS"
EOF
    chmod +x "$core_script"

    # 6. 保存配置
    cat > "$config_file" << EOF
REGION_CODE="${country_id}"
REGION_NAME="${city_id}"
REMOTE_PATH="${remote_path}"
BIND_IPV4="${bind_v4}"
BIND_IPV6="${bind_v6}"
WORK_MODE="${work_mode}"
INSTALL_DIR="${opt_dir}"
LOG_FILE="${log_dir}/freship.log"
EOF
    chown -R freship:freship "$conf_dir" "$log_dir" "$opt_dir"
    chmod 600 "$config_file"

    # 7. Systemd
    deploy_freship_systemd "$work_mode"
    success "FreshIP 部署完成。"
}

deploy_freship_systemd() {
    local mode=$1
    cat > /etc/systemd/system/freship-core@.service << EOF
[Unit]
Description=FreshIP Engine (%i)
After=network.target

[Service]
Type=oneshot
User=freship
Group=freship
ExecStart=/bin/bash /opt/freship/core/freship_core.sh %i
EOF

    cat > /etc/systemd/system/freship-core@.timer << EOF
[Unit]
Description=FreshIP Timer (%i)
[Timer]
OnBootSec=5min
OnUnitActiveSec=20min
RandomizedDelaySec=300
Unit=freship-core@%i.service
[Install]
WantedBy=timers.target
EOF

    systemctl daemon-reload
    [[ "$mode" == "ipv4_only" || "$mode" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
    [[ "$mode" == "ipv6_only" || "$mode" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
}

uninstall_freship() {
    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1
    rm -f /etc/systemd/system/freship-core@*
    systemctl daemon-reload
    rm -rf /opt/freship /etc/freship /var/log/freship
    id -u freship >/dev/null 2>&1 && userdel freship
    success "清理完成。"
}

manage_freship() {
    while true; do
        clear
        echo -e "🚀 [ FreshIP 管理 ]"
        echo " 1. 启动任务"
        echo " 2. 停止任务"
        echo " 3. 查阅运行日志"
        echo " 4. 卸载模块"
        echo " 0. 返回"
        read -p "选择: " opt
        case $opt in
            1) 
                source /etc/freship/freship.conf 2>/dev/null
                [[ "$WORK_MODE" == "ipv4_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
                [[ "$WORK_MODE" == "ipv6_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
                success "任务已开启。"; pause;;
            2) 
                systemctl disable --now freship-core@v4.timer freship-core@v6.timer 2>/dev/null
                info "任务已停止。"; pause;;
            3) 
                journalctl -u 'freship-core@*' --no-hostname -e
                pause
                ;;
            4) uninstall_freship; return;;
            0) break;;
        esac
    done
}
