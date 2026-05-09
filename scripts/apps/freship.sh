#!/bin/bash
# =========================================================
# FreshIP IP 养护 自动化部署 with Modern Logging (V3.2)
# =========================================================
# 准则: VIBEINSTRCT.md
# 架构: 昼夜感知、TLS 指纹链、每日活跃度随机化、热重载配置
# =========================================================

# ----------------- 内部工具函数 (私有) -----------------

_get_utc_offset() {
    local code=$1
    case "$code" in
        JP|KR) echo "+9" ;;
        SG|HK|TW|MY|PH|CN) echo "+8" ;;
        VN|TH|ID) echo "+7" ;;
        UK|IE|PT) echo "+0" ;;
        FR|DE|ES|IT|NL|BE|CH|PL|SE|NO|DK) echo "+1" ;;
        TR|SA|RU|UA|GR|RO) echo "+3" ;;
        US|CA) echo "-5" ;;
        AU) echo "+10" ;;
        NG|ZA) echo "+1" ;;
        *) echo "+0" ;;
    esac
}

_freship_select_region() {
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    info "正在同步全球节点地理索引..."
    
    if ! download_with_fallback "/tmp/freship_map.json" "${repo_raw}/data/map.json"; then
        err "网络环境波动，无法获取节点地图。"
        return 1
    fi

    mapfile -t c_ids   < <(jq -r '.continents[].countries[].id'           /tmp/freship_map.json)
    mapfile -t c_names < <(jq -r '.continents[].countries[].name'         /tmp/freship_map.json)
    mapfile -t c_kws   < <(jq -r '.continents[].countries[].keyword_file' /tmp/freship_map.json)

    echo -e "\n📍 请选择目标养护国家/地区 Country/Region："
    for i in "${!c_ids[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${c_names[$i]}"; done
    
    local c_sel
    read -rp "请输入序号 (默认 1): " c_sel
    c_sel=$(( ${c_sel:-1} - 1 ))
    [[ "$c_sel" -lt 0 || "$c_sel" -ge "${#c_ids[@]}" ]] && c_sel=0
    country_id="${c_ids[$c_sel]}"
    kw_filename="${c_kws[$c_sel]}"

    state_id=$(jq -r --arg c "$country_id" '.continents[].countries[]|select(.id==$c)|.states[0].id' /tmp/freship_map.json 2>/dev/null)
    city_id=$(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[0].id' /tmp/freship_map.json 2>/dev/null)
    city_name=$(jq -r --arg c "$country_id" --arg s "$state_id" '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[0].name' /tmp/freship_map.json 2>/dev/null)

    remote_path="${country_id}/${state_id}/${city_id}"
    rm -f /tmp/freship_map.json
    return 0
}

_freship_select_mode() {
    info "正在探测本机公网出口 IP 状态..."
    local detect_v4=$(curl -4 -s -m 5 api.ip.sb/ip 2>/dev/null || curl -4 -s -m 5 ifconfig.me 2>/dev/null || echo "")
    local detect_v6=$(curl -6 -s -m 5 api.ip.sb/ip 2>/dev/null || curl -6 -s -m 5 icanhazip.com 2>/dev/null || echo "")
    detect_v4=$(echo "$detect_v4" | tr -d '[:space:]')
    detect_v6=$(echo "$detect_v6" | tr -d '[:space:]')
    
    bind_v4=""; bind_v6=""; work_mode=""

    if [[ -n "$detect_v4" && -n "$detect_v6" ]]; then
        echo -e "\n检测到双栈 Dual-Stack IP 环境，请选择模式："
        echo "  1) 仅 IPv4 养护"
        echo "  2) 仅 IPv6 养护"
        echo "  3) 双栈独立并发养护"
        read -rp "请输入序号 (默认 3): " mode_sel
        case "${mode_sel:-3}" in
            1) work_mode="ipv4_only"; bind_v4="$detect_v4" ;;
            2) work_mode="ipv6_only"; bind_v6="$detect_v6" ;;
            *) work_mode="dual_stack"; bind_v4="$detect_v4"; bind_v6="$detect_v6" ;;
        esac
    elif [[ -n "$detect_v4" ]]; then
        work_mode="ipv4_only"; bind_v4="$detect_v4"
    elif [[ -n "$detect_v6" ]]; then
        work_mode="ipv6_only"; bind_v6="$detect_v6"
    fi
    return 0
}

# ----------------- 核心业务逻辑 -----------------

install_freship() {
    info "正在部署 FreshIP 拟人化引擎..."

    # 1. 环境准备
    safe_apt_install curl jq unzip file coreutils less bc || return 1
    create_system_user "freship"

    local opt_dir="/opt/freship"
    local conf_dir="/etc/freship"
    local config_file="${conf_dir}/freship.conf"

    [[ -d "$opt_dir" ]] && rm -rf "$opt_dir"
    mkdir -p "$opt_dir/bin" "$opt_dir/core" "$opt_dir/data/keywords" "$opt_dir/data/regions" "$conf_dir"

    # 2. 部署 TLS 引擎
    local arch=$(uname -m)
    local pkg_arch="x86_64-linux-gnu"
    [[ "$arch" == "aarch64" ]] && pkg_arch="aarch64-linux-gnu"
    
    info "正在同步 TLS 伪装特征库..."
    local dl_url="https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.${pkg_arch}.tar.gz"
    local tmp_tar="/tmp/freship_curl.tar.gz"
    if download_with_fallback "$tmp_tar" "$dl_url"; then
        tar -xzf "$tmp_tar" -C "${opt_dir}/bin" 2>/dev/null
        rm -f "$tmp_tar"
        chmod +x "${opt_dir}/bin/"* 2>/dev/null
    fi

    # 3. 配置交互
    local country_id city_name city_id remote_path kw_filename
    _freship_select_region || return 1
    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1
    local utc_offset=$(_get_utc_offset "$country_id")

    # 4. 资产拉取
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    download_with_fallback "${opt_dir}/data/regions/${city_id}.json" "${repo_raw}/data/regions/${remote_path}.json" || true
    download_with_fallback "${opt_dir}/data/keywords/${kw_filename}" "${repo_raw}/data/keywords/${kw_filename}" || true
    download_with_fallback "${opt_dir}/data/user_agents.txt" "${repo_raw}/data/user_agents.txt" || true

    # 5. 写入核心脚本
    local core_script="${opt_dir}/core/freship_core.sh"
    cat > "$core_script" << 'EOF'
#!/bin/bash
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CONFIG_FILE="/etc/freship/freship.conf"
[[ ! -f "$CONFIG_FILE" ]] && exit 1
source "$CONFIG_FILE"
INSTANCE_MODE=${1:-"global"}
DATA_DIR="${INSTALL_DIR}/data"
REGION_JSON=$(find "${DATA_DIR}/regions" -name "*.json" | head -n 1)

log() {
    local type=$1; local msg=$2; local icon="ℹ️"
    case "$type" in START) icon="🚀" ;; INFO) icon="📊" ;; SLEEP) icon="🌙" ;; SUCCESS) icon="✅" ;; ERROR) icon="❌" ;; ACTION) icon="🔗" ;; esac
    echo -e "[FreshIP] $icon | $(date '+%Y-%m-%d %H:%M:%S') | $INSTANCE_MODE | $REGION_CODE | $msg"
}

LOCAL_HOUR=$(date -u -d "${UTC_OFFSET:-+0} hours" +%H)
if [ "$LOCAL_HOUR" -ge 1 ] && [ "$LOCAL_HOUR" -le 6 ]; then
    log "SLEEP" "处于目标地区深夜 ($LOCAL_HOUR:00)，进入休眠模式。"
    exit 0
fi

DAILY_SEED=$(echo $(date +%Y%m%d) | cksum | awk '{print $1}')
ACTIVITY_LEVEL=$(( DAILY_SEED % 100 ))
if [ "$ACTIVITY_LEVEL" -lt 30 ] && [ $(( RANDOM % 100 )) -gt "$ACTIVITY_LEVEL" ]; then
    log "INFO" "今日活跃度低 ($ACTIVITY_LEVEL%)，当前轮次选择休假。"
    exit 0
fi

log "START" "启动养护任务 (活跃度: $ACTIVITY_LEVEL%)"
LOCK_FILE="/tmp/freship_${INSTANCE_MODE}.lock"; echo $$ > "$LOCK_FILE"; trap 'rm -f "$LOCK_FILE"' EXIT
BIND_IP=""; [[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="$BIND_IPV4"; [[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="$BIND_IPV6"
[[ -z "$BIND_IP" ]] && exit 1

CURL_BIN="curl"; TLS_MODE="Native"
for candidate in "curl_chrome124" "curl_chrome116" "curl_chrome110"; do
    if [ -f "${INSTALL_DIR}/bin/$candidate" ]; then CURL_BIN="${INSTALL_DIR}/bin/$candidate"; TLS_MODE="$candidate"; break; fi
done

UA_POOL="${DATA_DIR}/user_agents.txt"
SESSION_UA=$( [ -f "$UA_POOL" ] && shuf -n 1 "$UA_POOL" || echo "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124.0.0.0 Safari/537.36" )
IS_MOBILE=false; [[ "$SESSION_UA" =~ "Android" || "$SESSION_UA" =~ "iPhone" ]] && IS_MOBILE=true
PREV_URL=""

request() {
    local url=$1; local name=$2; local display_info=${3:-"URL: ${url:0:40}..."}
    local site_header="none"; [[ -n "$PREV_URL" ]] && site_header="same-origin"
    local cmd=( "$CURL_BIN" -s -L -o /dev/null -w "%{http_code}" --interface "$BIND_IP" -A "$SESSION_UA" )
    [[ -n "$PREV_URL" ]] && cmd+=( -e "$PREV_URL" ); cmd+=( -H "Sec-Fetch-Site: $site_header" )
    local code=$( "${cmd[@]}" "$url" )
    if [[ "$code" =~ ^2 ]]; then log "ACTION" "[$name] 响应码: $code | TLS: $TLS_MODE | $display_info"; else log "ERROR" "[$name] 响应码: $code | TLS: $TLS_MODE | $display_info"; fi
    PREV_URL="$url"; sleep $(( RANDOM % 5 + 2 ))
}

ROLL=$(( RANDOM % 100 ))
if [ "$ROLL" -lt 60 ]; then
    KW_FILE="${DATA_DIR}/keywords/kw_${REGION_CODE}.txt"; KW=$( [ -f "$KW_FILE" ] && shuf -n 1 "$KW_FILE" || echo "Debian Linux" )
    ENCODED_KW=$(jq -rn --arg x "$KW" '$x|@uri')
    request "https://www.google.com/search?q=${ENCODED_KW}" "SEARCH" "关键字: $KW"
elif [ "$ROLL" -lt 85 ]; then
    if [ $(( RANDOM % 100 )) -lt 70 ]; then URL=$( jq -r '.trust_module.white_urls[]' "$REGION_JSON" | shuf -n 1 ); request "$URL" "NEWS_WHITE"
    else URL=$( jq -r '.trust_module.static_urls[]' "$REGION_JSON" | shuf -n 1 ); request "$URL" "NEWS_STATIC"; fi
elif [ "$ROLL" -lt 95 ]; then request "https://www.google.com/maps/search/restaurants+near+me" "MAPS"
else if [ "$IS_MOBILE" = true ]; then request "http://connectivitycheck.gstatic.com/generate_204" "PROBE_MOBILE"
    else request "https://www.google.com/imghp?hl=zh-CN" "PROBE_DESKTOP"; fi
fi
log "SUCCESS" "养护流程执行完毕。"
EOF
    chmod +x "$core_script"

    # 6. 保存配置
    cat > "$config_file" << EOF
REGION_CODE="${country_id}"
REGION_NAME="${city_name}"
UTC_OFFSET="${utc_offset}"
BIND_IPV4="${bind_v4}"
BIND_IPV6="${bind_v6}"
WORK_MODE="${work_mode}"
INSTALL_DIR="${opt_dir}"
EOF
    chown -R freship:freship "$conf_dir" "$opt_dir"
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
StandardOutput=journal
StandardError=journal
SyslogIdentifier=freship
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

reconfigure_freship() {
    info "正在进入 FreshIP 热重载配置流程..."
    source /etc/freship/freship.conf 2>/dev/null
    
    # 停止任务
    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1
    
    # 重新选择
    local country_id city_name city_id remote_path kw_filename
    _freship_select_region || return 1
    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1
    local utc_offset=$(_get_utc_offset "$country_id")
    
    # 资产更新
    local repo_raw="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
    download_with_fallback "/opt/freship/data/regions/${city_id}.json" "${repo_raw}/data/regions/${remote_path}.json" || true
    download_with_fallback "/opt/freship/data/keywords/${kw_filename}" "${repo_raw}/data/keywords/${kw_filename}" || true
    
    # 覆写配置
    cat > /etc/freship/freship.conf << EOF
REGION_CODE="${country_id}"
REGION_NAME="${city_name}"
UTC_OFFSET="${utc_offset}"
BIND_IPV4="${bind_v4}"
BIND_IPV6="${bind_v6}"
WORK_MODE="${work_mode}"
INSTALL_DIR="/opt/freship"
EOF
    
    # 重新部署定时器
    deploy_freship_systemd "$work_mode"
    success "配置更新已生效，任务已重新上线。"
}

uninstall_freship() {
    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1
    rm -f /etc/systemd/system/freship-core@*
    systemctl daemon-reload
    rm -rf /opt/freship /etc/freship
    id -u freship >/dev/null 2>&1 && userdel freship
    success "FreshIP 已卸载。"
}

manage_freship() {
    while true; do
        source /etc/freship/freship.conf 2>/dev/null
        
        # 智能状态感知
        local is_active=false
        if systemctl is-active --quiet freship-core@v4.timer || systemctl is-active --quiet freship-core@v6.timer; then
            is_active=true
        fi
        
        # 菜单渲染
        ui_draw_header "FreshIP 养护管理" "App > FreshIP"
        
        local toggle_label="🔄 启动养护任务"
        local toggle_status="${DIM}○ 已停止${NC}"
        if [ "$is_active" = true ]; then
            toggle_label="🔄 停止养护任务"
            toggle_status="${GREEN}●${NC} ${DIM}运行中${NC}"
        fi
        
        ui_draw_item "1" "$toggle_label" "$toggle_status"
        ui_draw_item "2" "⚙️ 修改模块设置"
        ui_draw_item "3" "📜 查阅运行日志"
        ui_draw_item "4" "🗑️ 卸载模块"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回"
        
        echo ""
        read -p " >>> 选择: " opt
        case $opt in
            1)
                if [ "$is_active" = true ]; then
                    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1
                    info "任务已安全挂起。"
                else
                    [[ "$WORK_MODE" == "ipv4_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
                    [[ "$WORK_MODE" == "ipv6_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
                    success "任务已成功启动。"
                fi
                pause ;;
            2) reconfigure_freship; pause ;;
            3) 
                echo -e "${CYAN}--- 最近 100 条运行日志 (Q 退出) ---${NC}"
                journalctl -t freship --no-hostname -n 100 --no-pager
                echo ""
                pause 
                ;;
            4) uninstall_freship; return ;;
            0) break ;;
        esac
    done
}
