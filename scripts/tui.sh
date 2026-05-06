#!/bin/bash
# =========================================================
# TUI 交互式菜单与渲染引擎
# =========================================================

# ----------------- UI 渲染引擎 (核心) -----------------

# [内部] 获取字符串视觉宽度 (适配 中文/Emoji/ANSI)
_ui_visual_len() {
    local str=$1
    # 1. 移除 ANSI 转义序列
    local clean_str=$(echo -e "$str" | sed 's/\x1b\[[0-9;]*m//g')
    # 2. 移除变体选择符 (如 VS16)，避免干扰宽度计算
    clean_str=$(echo -e "$clean_str" | sed 's/\xEF\xB8\x8F//g')
    # 3. 使用字节与字符差值算法计算 CJK 宽度补偿
    local char_count=${#clean_str}
    local byte_count=$(printf "%s" "$clean_str" | wc -c)
    # 视觉宽度 = 字符数 + (字节数 - 字符数) / 2 
    echo $(( char_count + (byte_count - char_count) / 2 ))
}

# 绘制菜单页眉 (居中对齐)
ui_draw_header() {
    local title=$1
    local subtitle=$2
    local total_inner_width=50
    
    clear
    echo -e "${CYAN}┌──────────────────────────────────────────────────┐${NC}"
    
    # 标题居中处理
    local t_len=$(_ui_visual_len "$title")
    local t_pad_total=$(( total_inner_width - t_len ))
    [[ $t_pad_total -lt 0 ]] && t_pad_total=0
    local t_pad_l=$(( t_pad_total / 2 ))
    local t_pad_r=$(( t_pad_total - t_pad_l ))
    printf "${CYAN}│${NC}%*s${BOLD}%s${NC}%*s${CYAN}│${NC}\n" "$t_pad_l" "" "$title" "$t_pad_r" ""
    
    # 副标题居中处理
    if [[ -n "$subtitle" ]]; then
        local s_len=$(_ui_visual_len "$subtitle")
        local s_pad_total=$(( total_inner_width - s_len ))
        [[ $s_pad_total -lt 0 ]] && s_pad_total=0
        local s_pad_l=$(( s_pad_total / 2 ))
        local s_pad_r=$(( s_pad_total - s_pad_l ))
        printf "${CYAN}│${NC}%*s${DIM}%s${NC}%*s${CYAN}│${NC}\n" "$s_pad_l" "" "$subtitle" "$s_pad_r" ""
    fi
    echo -e "${CYAN}└──────────────────────────────────────────────────┘${NC}"
}

# 绘制菜单项 (三列固定栅格布局)
ui_draw_item() {
    local id=$1
    local raw_desc=$2
    local status=${3:-}
    local total_width=50
    
    # 1. 逻辑分离：提取图标与文字标签
    local icon=$(echo "$raw_desc" | awk '{print $1}')
    local label=$(echo "$raw_desc" | cut -d' ' -f2-)
    
    # 处理没有图标的特殊项 (如 0. 退出)
    if [[ "$icon" =~ ^[0-9]+$ || "$label" == "" ]]; then
        icon=""
        label="$raw_desc"
    fi

    # 2. 宽度计算
    local i_len=0
    [[ -n "$icon" ]] && i_len=$(_ui_visual_len "$icon")
    local l_len=$(_ui_visual_len "$label")
    local s_len=0
    [[ -n "$status" ]] && s_len=$(_ui_visual_len "$status")

    # 3. 分阶段绘制 (列对齐)
    
    # [列 A: 编号] 固定宽度 4: " XX."
    printf " %2s." "$id"
    
    # [列 B: 图标] 固定视觉宽度 5
    if [[ -n "$icon" ]]; then
        local i_pad=$(( 5 - i_len ))
        [[ $i_pad -lt 0 ]] && i_pad=0
        printf "  %s%*s" "$icon" "$i_pad" ""
    else
        printf "       "
    fi

    # [列 C: 标签 & 状态] 
    if [[ -n "$status" ]]; then
        # 弹性空间计算：总宽 - 编号(4) - 图标区(7) - 标签宽 - 状态宽
        local padding=$(( total_width - 4 - 7 - l_len - s_len ))
        [[ $padding -lt 1 ]] && padding=1
        printf "%s%*s%s\n" "$label" "$padding" "" "$status"
    else
        printf "%s\n" "$label"
    fi
}

# 绘制分隔线
ui_draw_sep() {
    echo -e "${DIM} ──────────────────────────────────────────────────${NC}"
}

# ----------------- 状态感知引擎 -----------------

# 获取命令状态图标
get_status() {
    local cmd=$1
    local dir_name="${cmd%-core}" 
    local is_installed=false
    
    if command -v "$cmd" >/dev/null 2>&1 || [[ -f "/usr/bin/$cmd" ]] || [[ -f "/usr/local/bin/$cmd" ]] || [[ -f "/opt/$cmd/$cmd" ]] || [[ -f "/opt/$dir_name/$cmd" ]] || [[ -f "/opt/freship/opt/core/freship_core.sh" && "$cmd" == "freship" ]]; then
        is_installed=true
    fi

    if [[ "$is_installed" == "true" ]]; then
        echo -e "${GREEN}●${NC} ${DIM}已就绪${NC}"
    else
        echo -e "${DIM}○ 未部署${NC}"
    fi
}

# 获取组合状态图标
get_combined_status() {
    local is_installed=false
    for cmd in "$@"; do
        if command -v "$cmd" >/dev/null 2>&1 || [[ -f "/usr/bin/$cmd" ]] || [[ -f "/opt/$cmd/$cmd" ]]; then
            is_installed=true
            break
        fi
    done

    if [[ "$is_installed" == "true" ]]; then
        echo -e "${GREEN}●${NC} ${DIM}已就绪${NC}"
    else
        echo -e "${DIM}○ 未部署${NC}"
    fi
}

# ----------------- 子菜单管理模板 -----------------

handle_submenu() {
    local app_name=$1
    local install_func=$2
    local uninstall_func=$3
    while true; do
        ui_draw_header "$app_name 管理" "Main > $app_name"
        ui_draw_item "1" "✨ 安装 / 更新"
        ui_draw_item "2" "🗑️ 卸载并清理"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回上级菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) $install_func; pause; break;;
            2) 
                read -p "确定要移除 $app_name 吗？[y/N]: " confirm
                if [[ "$confirm" =~ ^[Yy]$ ]]; then
                    $uninstall_func; pause; break
                fi
                ;;
            0) break;;
            *) warn "无效输入。";;
        esac
    done
}

# ----------------- 业务子菜单 -----------------

handle_warp_submenu() {
    while true; do
        ui_draw_header "代理与出站管理" "Main > Proxy Stack"
        ui_draw_item "1" "🚀 CF WARP 官方客户端" "$(get_status warp-cli)"
        ui_draw_item "2" "🛰️ Usque MASQUE 协议客户端" "$(get_status usque)"
        ui_draw_item "3" "📝 生成 Xray WG 出站 JSON"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) handle_submenu "WARP" install_warp uninstall_warp;;
            2) handle_submenu "Usque" install_usque uninstall_usque;;
            3) generate_warp_xray; pause;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

handle_go_submenu() {
    while true; do
        ui_draw_header "GoLang 环境与应用" "Main > Go Stack"
        ui_draw_item "1" "🐹 Go 语言环境 SDK" "$(get_status go)"
        ui_draw_item "2" "🌐 Caddy 自编译插件版" "$(get_status caddy)"
        ui_draw_item "3" "📡 DERP 隐身转发节点" "$(get_status derper)"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) handle_submenu "Go SDK" install_go uninstall_go;;
            2) handle_submenu "Caddy" install_caddy uninstall_caddy;;
            3) handle_submenu "DERP" install_derper uninstall_derper;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

handle_rust_submenu() {
    while true; do
        ui_draw_header "Rust 环境与应用" "Main > Rust Stack"
        ui_draw_item "1" "🦀 Rust 语言环境 rustup" "$(get_status rustc)"
        ui_draw_item "2" "🔗 Realm 转发服务器" "$(get_status realm)"
        ui_draw_item "3" "🌐 Ferron Web 服务器" "$(get_status ferron)"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) handle_submenu "Rust SDK" install_rust uninstall_rust;;
            2) handle_submenu "Realm" install_realm uninstall_realm;;
            3) handle_submenu "Ferron" install_ferron uninstall_ferron;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

handle_devops_submenu() {
    while true; do
        ui_draw_header "运维与终端工具" "Main > DevOps Tools"
        ui_draw_item "1" "🐟 Fish Shell 现代化 Shell" "$(get_status fish)"
        ui_draw_item "2" "📝 Micro Editor 文本编辑器" "$(get_status micro)"
        ui_draw_item "3" "📜 Acme.sh 证书自动化工具" "$(get_status acme.sh)"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) handle_submenu "Fish" install_fish uninstall_fish;;
            2) handle_submenu "Micro" install_micro uninstall_micro;;
            3) handle_submenu "Acme.sh" install_acme uninstall_acme;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

handle_maintenance_submenu() {
    while true; do
        ui_draw_header "脚本维护" "Main > Maintenance"
        ui_draw_item "1" "🆙 检查并同步最新版本"
        ui_draw_item "2" "🗑️ 彻底卸载脚本及资产"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回主菜单"
        echo ""
        read -p " >>> 选择: " sub_choice
        case $sub_choice in
            1) script_update; pause;;
            2) script_uninstall;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

# ----------------- 主菜单入口 -----------------

show_main_menu() {
    while true; do
        hash -r 
        local net_status_text=""
        [[ "$IS_CN_REGION" == "true" ]] && net_status_text="${YELLOW}中国大陆 (镜像加速)${NC}" || net_status_text="${GREEN}海外地区 (直连模式)${NC}"
        
        ui_draw_header "Debian Optimizer & Manager" "Ver: $VERSION_ID | $net_status_text"
        
        ui_draw_item "1" "⚡ 一键系统级基础优化"
        ui_draw_item "2" "🌐 路由转发模式控制" "$(get_ip_forward_status)"
        ui_draw_sep
        ui_draw_item "3" "🛠️ 运维与终端工具集" "$(get_combined_status fish micro acme.sh)"
        ui_draw_item "4" "🐳 Docker 引擎与编排" "$(get_status docker)"
        ui_draw_item "5" "🦀 Rust 环境与应用" "$(get_status rustc)"
        ui_draw_item "6" "🐹 GoLang 环境与应用" "$(get_status go)"
        ui_draw_item "7" "🛰️ Xray Core 节点管理" "$(get_status xray)"
        ui_draw_item "8" "🚀 WARP & Usque 代理栈" "$(get_combined_status warp-cli usque)"
        ui_draw_item "9" "🔗 Easytier 虚拟组网" "$(get_status easytier-core)"
        ui_draw_item "10" "🌊 Tailscale 虚拟组网" "$(get_status tailscale)"
        ui_draw_item "11" "🌱 FreshIP IP 养护" "$(get_status freship)"
        ui_draw_sep
        ui_draw_item "12" "⚙️ 脚本维护 (更新/卸载)"
        ui_draw_item "0" "🔙 退出脚本"
        ui_draw_sep
        
        echo ""
        read -p " >>> 请输入指令: " choice
        case $choice in
            1) run_base_optimization; pause;;
            2) toggle_ip_forwarding; pause;;
            3) handle_devops_submenu;;
            4) handle_submenu "Docker" install_docker uninstall_docker;;
            5) handle_rust_submenu;;
            6) handle_go_submenu;;
            7) handle_submenu "Xray" install_xray uninstall_xray;;
            8) handle_warp_submenu;;
            9) handle_submenu "Easytier" install_easytier uninstall_easytier;;
            10) handle_submenu "Tailscale" install_tailscale uninstall_tailscale;;
            11) 
                if [[ "$(get_status freship)" == *"未部署"* ]]; then
                    install_freship; pause
                else
                    manage_freship
                fi
                ;;
            12) handle_maintenance_submenu;;
            0) break;;
            *) warn "无效指令: $choice"; sleep 0.5;;
        esac
    done
}
