#!/bin/bash
# =========================================================
# FreshIP IP 养护 v4 — 探针优先、多模块引擎
# =========================================================

FRESHIP_REPO_RAW="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"

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

_freship_target_cc() {
    local code=$1
    code="${code%%-*}"
    [[ "$code" == "UK" ]] && code="GB"
    echo "$code"
}

_freship_deploy_core_scripts() {
    local opt_dir=$1
    local core_dir="${opt_dir}/core"
    mkdir -p "$core_dir" "${opt_dir}/logs" "${opt_dir}/data/cookies"
    local scripts=(
        freship_lib.sh
        freship_probe.sh
        freship_mod_google.sh
        freship_mod_trust.sh
        freship_runner.sh
        freship_updater.sh
        freship_core.sh
    )
    local s
    for s in "${scripts[@]}"; do
        render_template "templates/apps/freship/${s}" "${core_dir}/${s}" "INSTALL_DIR=${opt_dir}" || return 1
        chmod +x "${core_dir}/${s}"
    done
}

_freship_pull_assets() {
    local opt_dir=$1 remote_path=$2 kw_filename=$3
    local region_dest="${opt_dir}/data/regions/${remote_path}.json"
    mkdir -p "$(dirname "$region_dest")" "${opt_dir}/data/keywords"
    download_with_fallback "$region_dest" "${FRESHIP_REPO_RAW}/data/regions/${remote_path}.json" || true
    download_with_fallback "${opt_dir}/data/keywords/${kw_filename}" "${FRESHIP_REPO_RAW}/data/keywords/${kw_filename}" || true
    download_with_fallback "${opt_dir}/data/user_agents.txt" "${FRESHIP_REPO_RAW}/data/user_agents.txt" || true
    download_with_fallback "${opt_dir}/data/map.json" "${FRESHIP_REPO_RAW}/data/map.json" || true
}

_freship_render_conf() {
    local config_file=$1 opt_dir=$2
    render_template "templates/apps/freship/freship.conf" "$config_file" \
        "REGION_CODE=${country_id}" \
        "REGION_NAME=${city_name}" \
        "REGION_PATH=${remote_path}" \
        "KW_FILE=${kw_filename}" \
        "TARGET_CC=$(_freship_target_cc "$country_id")" \
        "UTC_OFFSET=${utc_offset}" \
        "BIND_IPV4=${bind_v4}" \
        "BIND_IPV6=${bind_v6}" \
        "WORK_MODE=${work_mode}" \
        "FRESHIP_DIR=${opt_dir}"
}

_freship_clear_state() {
    rm -f /etc/freship/state/v4.state /etc/freship/state/v6.state 2>/dev/null || true
}

_freship_read_instance_state() {
    local inst=$1 key=$2 default=${3:-}
    local sf="/etc/freship/state/${inst}.state"
    [[ ! -f "$sf" ]] && { echo "$default"; return; }
    local line
    line=$(grep "^${key}=" "$sf" 2>/dev/null | tail -n 1)
    [[ -z "$line" ]] && { echo "$default"; return; }
    echo "${line#${key}=}"
}

_freship_draw_status_panel() {
    local work_mode=${WORK_MODE:-dual_stack}
    local rows=() inst mode score
    for inst in v4 v6; do
        [[ "$inst" == "v4" && "$work_mode" == "ipv6_only" ]] && continue
        [[ "$inst" == "v6" && "$work_mode" == "ipv4_only" ]] && continue
        mode=$(_freship_read_instance_state "$inst" "RUN_MODE" "")
        score=$(_freship_read_instance_state "$inst" "LAST_SCORE" "")
        [[ -z "$mode" && -z "$score" ]] && continue
        rows+=("${inst}|${mode}|${score}")
    done
    [[ ${#rows[@]} -eq 0 ]] && return

    local inner_w=50
    local row inst label mode_txt score_txt mode_c score_c content pad
    echo -e "${CYAN}┌──────────────────────────────────────────────────┐${NC}"
    content=$(printf "  ${BOLD}实例状态${NC}")
    pad=$(( inner_w - $(_ui_visual_len "$content") ))
    [[ "$pad" -lt 0 ]] && pad=0
    printf "${CYAN}│${NC}%s%*s${CYAN}│${NC}\n" "$content" "$pad" ""
    echo -e "${CYAN}├──────────────────────────────────────────────────┤${NC}"
    for row in "${rows[@]}"; do
        IFS='|' read -r inst mode score <<< "$row"
        if [[ "$inst" == "v4" ]]; then
            label="IPv4"
        else
            label="IPv6"
        fi
        case "$mode" in
            maintain) mode_txt="仅自检"; mode_c="${CYAN}" ;;
            simulate) mode_txt="模拟养护"; mode_c="${YELLOW}" ;;
            *) mode_txt="—"; mode_c="${DIM}" ;;
        esac
        case "$score" in
            ok) score_txt="达标"; score_c="${GREEN}" ;;
            cn) score_txt="送中"; score_c="${RED}" ;;
            drift) score_txt="漂移"; score_c="${YELLOW}" ;;
            fail) score_txt="异常"; score_c="${RED}" ;;
            *) score_txt="${score:-—}"; score_c="${DIM}" ;;
        esac
        content=$(printf "  ${BOLD}%-6s${NC}  ${mode_c}%-8s${NC}  ${DIM}·${NC}  ${score_c}%s${NC}" \
            "$label" "$mode_txt" "$score_txt")
        pad=$(( inner_w - $(_ui_visual_len "$content") ))
        [[ "$pad" -lt 0 ]] && pad=0
        printf "${CYAN}│${NC}%s%*s${CYAN}│${NC}\n" "$content" "$pad" ""
    done
    echo -e "${CYAN}└──────────────────────────────────────────────────┘${NC}"
}

_freship_select_region() {
    local map_cache=${1:-}
    info "正在同步全球节点地理索引..."

    if ! download_with_fallback "/tmp/freship_map.json" "${FRESHIP_REPO_RAW}/data/map.json"; then
        if [[ -n "$map_cache" && -f "$map_cache" ]]; then
            cp "$map_cache" /tmp/freship_map.json
            warn "网络不可用，使用本地离线地图。"
        else
            err "网络环境波动，无法获取节点地图。"
            return 1
        fi
    fi

    if [[ -n "$map_cache" ]]; then
        mkdir -p "$(dirname "$map_cache")"
        cp /tmp/freship_map.json "$map_cache"
        local map_ver
        map_ver=$(jq -r '.version // "unknown"' /tmp/freship_map.json 2>/dev/null)
        info "地图版本: ${map_ver}"
    fi

    mapfile -t c_ids   < <(jq -r '.continents[].countries[].id'           /tmp/freship_map.json)
    mapfile -t c_names < <(jq -r '.continents[].countries[].name'         /tmp/freship_map.json)
    mapfile -t c_kws   < <(jq -r '.continents[].countries[].keyword_file' /tmp/freship_map.json)

    echo -e "\n📍 [1/2] 请选择目标养护国家/地区 Country/Region："
    for i in "${!c_ids[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${c_names[$i]}"; done

    local c_sel
    read -rp "请输入序号 (默认 1): " c_sel
    c_sel=$(( ${c_sel:-1} - 1 ))
    [[ "$c_sel" -lt 0 || "$c_sel" -ge "${#c_ids[@]}" ]] && c_sel=0
    country_id="${c_ids[$c_sel]}"
    kw_filename="${c_kws[$c_sel]}"

    mapfile -t flat_cities < <(jq -r --arg c "$country_id" '
        .continents[].countries[] | select(.id == $c) |
        .states[] as $s |
        $s.cities[] |
        [$s.id, .id, .name, $s.name] | join("|")
    ' /tmp/freship_map.json 2>/dev/null)

    local city_count="${#flat_cities[@]}"

    if [[ "$city_count" -eq 0 ]]; then
        err "未能解析该国城市数据，请检查 map.json 格式。"
        rm -f /tmp/freship_map.json
        return 1
    elif [[ "$city_count" -eq 1 ]]; then
        IFS='|' read -r state_id city_id city_name s_name <<< "${flat_cities[0]}"
        info "城市已自动确认：${city_name}"
    else
        echo -e "\n🏙️  [2/2] 请选择目标城市 City："
        for i in "${!flat_cities[@]}"; do
            IFS='|' read -r _sid _cid c_nm s_nm <<< "${flat_cities[$i]}"
            local display_name="$c_nm"
            [[ "$_sid" != "Default" ]] && display_name="${c_nm}  [${s_nm}]"
            printf "  %2d) %s\n" "$(( i+1 ))" "$display_name"
        done
        local city_sel
        read -rp "请输入序号 (默认 1): " city_sel
        city_sel=$(( ${city_sel:-1} - 1 ))
        [[ "$city_sel" -lt 0 || "$city_sel" -ge "$city_count" ]] && city_sel=0
        IFS='|' read -r state_id city_id city_name s_name <<< "${flat_cities[$city_sel]}"
    fi

    remote_path="${country_id}/${state_id}/${city_id}"
    rm -f /tmp/freship_map.json
    return 0
}

_freship_select_mode() {
    info "正在探测本机公网出口 IPv4..."
    local detect_v4
    detect_v4=$(curl -4 -s -m 5 api.ip.sb/ip 2>/dev/null || curl -4 -s -m 5 ifconfig.me 2>/dev/null || echo "")
    detect_v4=$(echo "$detect_v4" | tr -d '[:space:]')

    info "正在探测本机公网出口 IPv6（双轨检测）..."
    local detect_v6=""

    mapfile -t v6_candidates < <(
        ip -6 addr show scope global 2>/dev/null \
        | grep "inet6" \
        | grep -v "temporary\|deprecated" \
        | grep -oP '(?<=inet6 )[\da-f:]+(?=/)' \
        | grep -v '^::1' \
        | grep -v '^fe80'
    )

    for v6_addr in "${v6_candidates[@]}"; do
        if curl -6 -s -m 6 -o /dev/null \
               --interface "$v6_addr" \
               "https://ipv6.google.com" 2>/dev/null; then
            detect_v6="$v6_addr"
            info "IPv6 本地发现并验证成功：$detect_v6"
            break
        fi
    done

    if [[ -z "$detect_v6" ]]; then
        detect_v6=$(curl -6 -s -m 8 api.ip.sb/ip 2>/dev/null \
                 || curl -6 -s -m 8 icanhazip.com 2>/dev/null \
                 || echo "")
        detect_v6=$(echo "$detect_v6" | tr -d '[:space:]')
        [[ -n "$detect_v6" && "$detect_v6" != *:* ]] && detect_v6=""
        [[ -n "$detect_v6" ]] && info "IPv6 外部 API 检测成功：$detect_v6"
    fi

    bind_v4=""; bind_v6=""; work_mode=""

    if [[ -n "$detect_v4" && -n "$detect_v6" ]]; then
        echo -e "\n检测到双栈 Dual-Stack IP 环境，请选择模式："
        echo "  1) 仅 IPv4 养护  (${detect_v4})"
        echo "  2) 仅 IPv6 养护  (${detect_v6})"
        echo "  3) 双栈独立并发养护"
        read -rp "请输入序号 (默认 3): " mode_sel
        case "${mode_sel:-3}" in
            1) work_mode="ipv4_only"; bind_v4="$detect_v4" ;;
            2) work_mode="ipv6_only"; bind_v6="$detect_v6" ;;
            *) work_mode="dual_stack"; bind_v4="$detect_v4"; bind_v6="$detect_v6" ;;
        esac
    elif [[ -n "$detect_v4" ]]; then
        work_mode="ipv4_only"; bind_v4="$detect_v4"
        info "仅检测到 IPv4：$detect_v4"
    elif [[ -n "$detect_v6" ]]; then
        work_mode="ipv6_only"; bind_v6="$detect_v6"
        info "仅检测到 IPv6：$detect_v6"
    else
        err "未能检测到可用的 IPv4/IPv6 公网出口，请检查网络后重试。"
        return 1
    fi
    return 0
}

# ----------------- 核心业务逻辑 -----------------

install_freship() {
    info "正在部署 FreshIP v4 引擎..."

    safe_apt_install curl jq unzip file coreutils less bc || return 1
    create_system_user "freship"

    local opt_dir="/opt/freship"
    local conf_dir="/etc/freship"
    local config_file="${conf_dir}/freship.conf"

    [[ -d "$opt_dir" ]] && rm -rf "$opt_dir"
    mkdir -p "$opt_dir/bin" "$opt_dir/core" "$opt_dir/data/keywords" \
        "$opt_dir/data/regions" "$opt_dir/data/cookies" "$opt_dir/logs" \
        "${conf_dir}/state"

    local arch pkg_arch="x86_64-linux-gnu"
    arch=$(uname -m)
    [[ "$arch" == "aarch64" ]] && pkg_arch="aarch64-linux-gnu"

    info "正在同步 TLS 伪装特征库..."
    local dl_url="https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.${pkg_arch}.tar.gz"
    local tmp_tar="/tmp/freship_curl.tar.gz"
    if download_with_fallback "$tmp_tar" "$dl_url"; then
        tar -xzf "$tmp_tar" -C "${opt_dir}/bin" 2>/dev/null
        rm -f "$tmp_tar"
        chmod +x "${opt_dir}/bin/"* 2>/dev/null || true
    fi

    local country_id city_name city_id remote_path kw_filename
    _freship_select_region "${opt_dir}/data/map.json" || return 1
    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1
    local utc_offset
    utc_offset=$(_get_utc_offset "$country_id")

    _freship_pull_assets "$opt_dir" "$remote_path" "$kw_filename"
    _freship_deploy_core_scripts "$opt_dir" || return 1
    _freship_render_conf "$config_file" "$opt_dir" || return 1

    chown -R freship:freship "$conf_dir" "$opt_dir"
    chmod 600 "$config_file"
    _freship_clear_state

    deploy_freship_systemd "$work_mode"
    success "FreshIP v4 部署完成。"
}

deploy_freship_systemd() {
    local mode=$1
    render_template "templates/apps/freship/freship-core@.service" "/etc/systemd/system/freship-core@.service"
    render_template "templates/apps/freship/freship-core@.timer" "/etc/systemd/system/freship-core@.timer"
    render_template "templates/apps/freship/freship-updater.service" "/etc/systemd/system/freship-updater.service"
    render_template "templates/apps/freship/freship-updater.timer" "/etc/systemd/system/freship-updater.timer"

    systemctl daemon-reload
    systemctl enable --now freship-updater.timer
    [[ "$mode" == "ipv4_only" || "$mode" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
    [[ "$mode" == "ipv6_only" || "$mode" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
}

reconfigure_freship() {
    info "正在进入 FreshIP 热重载配置流程..."
    # shellcheck source=/dev/null
    source /etc/freship/freship.conf 2>/dev/null

    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1

    local country_id city_name city_id remote_path kw_filename
    _freship_select_region "/opt/freship/data/map.json" || return 1
    local bind_v4 bind_v6 work_mode
    _freship_select_mode || return 1
    local utc_offset
    utc_offset=$(_get_utc_offset "$country_id")

    _freship_pull_assets "/opt/freship" "$remote_path" "$kw_filename"
    _freship_deploy_core_scripts "/opt/freship" || return 1
    _freship_render_conf "/etc/freship/freship.conf" "/opt/freship" || return 1
    _freship_clear_state

    chown -R freship:freship /etc/freship /opt/freship
    chmod 600 /etc/freship/freship.conf

    deploy_freship_systemd "$work_mode"
    success "配置更新已生效，状态已重置为 simulate。"
}

sync_freship_data() {
    if [[ ! -x /opt/freship/core/freship_updater.sh ]]; then
        err "FreshIP 未安装或 updater 缺失。"
        return 1
    fi
    info "正在手动同步热数据..."
    if sudo -u freship bash /opt/freship/core/freship_updater.sh; then
        success "热数据同步完成。"
    else
        err "热数据同步失败，请查阅日志。"
        return 1
    fi
}

uninstall_freship() {
    systemctl disable --now freship-core@v4.timer freship-core@v6.timer freship-updater.timer >/dev/null 2>&1 || true
    rm -f /etc/systemd/system/freship-core@* /etc/systemd/system/freship-updater.*
    systemctl daemon-reload >/dev/null 2>&1 || true
    rm -rf /opt/freship /etc/freship
    id -u freship >/dev/null 2>&1 && userdel freship 2>/dev/null || true
    success "FreshIP 已卸载。"
}

manage_freship() {
    while true; do
        # shellcheck source=/dev/null
        source /etc/freship/freship.conf 2>/dev/null

        local is_active=false toggle_label toggle_status
        if systemctl is-active --quiet freship-core@v4.timer \
            || systemctl is-active --quiet freship-core@v6.timer; then
            is_active=true
        fi

        ui_draw_header "FreshIP 养护管理" "App > FreshIP"

        _freship_draw_status_panel
        echo ""

        toggle_label="🔄 启动养护任务"
        toggle_status="${DIM}○ 已停止${NC}"
        if [[ "$is_active" == true ]]; then
            toggle_label="🔄 停止养护任务"
            toggle_status="${GREEN}●${NC} ${DIM}运行中${NC}"
        fi

        ui_draw_item "1" "$toggle_label" "$toggle_status"
        ui_draw_item "2" "⚙️ 修改模块设置"
        ui_draw_item "3" "📥 手动同步热数据"
        ui_draw_item "4" "📜 查阅运行日志"
        ui_draw_item "5" "🗑️ 卸载模块"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回"

        echo ""
        read -rp " >>> 选择: " opt
        case $opt in
            1)
                if [[ "$is_active" == true ]]; then
                    systemctl disable --now freship-core@v4.timer freship-core@v6.timer >/dev/null 2>&1
                    info "任务已安全挂起。"
                else
                    [[ "$WORK_MODE" == "ipv4_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v4.timer
                    [[ "$WORK_MODE" == "ipv6_only" || "$WORK_MODE" == "dual_stack" ]] && systemctl enable --now freship-core@v6.timer
                    systemctl enable --now freship-updater.timer 2>/dev/null || true
                    success "任务已成功启动。"
                fi
                pause ;;
            2) reconfigure_freship; pause ;;
            3) sync_freship_data; pause ;;
            4)
                echo -e "${CYAN}--- 最近 100 条运行日志 ---${NC}"
                journalctl -t freship --no-hostname -n 100 --no-pager -o short-iso
                echo ""
                pause ;;
            5) uninstall_freship; return ;;
            0) break ;;
        esac
    done
}
