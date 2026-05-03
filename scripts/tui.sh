#!/bin/bash
# =========================================================
# TUI 交互式菜单与状态感知引擎
# =========================================================

# ----------------- 状态感知函数 -----------------

# 检测二进制文件是否存在于系统关键路径或安装目录
get_status() {
    local cmd=$1
    local dir_name="${cmd%-core}" 
    if command -v "$cmd" >/dev/null 2>&1 || [[ -f "/usr/bin/$cmd" ]] || [[ -f "/usr/local/bin/$cmd" ]] || [[ -f "/opt/$cmd/$cmd" ]] || [[ -f "/opt/$dir_name/$cmd" ]]; then
        echo -e "${GREEN}[已安装]${NC}"
    else
        echo -e "${YELLOW}[未安装]${NC}"
    fi
}

# 组合检测：只要集合中任意一个命令存在即视为已安装
get_combined_status() {
    for cmd in "$@"; do
        if command -v "$cmd" >/dev/null 2>&1 || [[ -f "/usr/bin/$cmd" ]] || [[ -f "/opt/$cmd/$cmd" ]]; then
            echo -e "${GREEN}[已安装]${NC}"
            return 0
        fi
    done
    echo -e "${YELLOW}[未安装]${NC}"
}

# ----------------- 子菜单管理模板 -----------------

handle_submenu() {
    local app_name=$1
    local install_func=$2
    local uninstall_func=$3
    while true; do
        echo -e "\n--- 【 $app_name 深度管理 】 ---"
        echo "1. 执行安装 / 版本更新"
        echo "2. 深度卸载并清理残留"
        echo "0. 返回上级菜单"
        read -p "请输入数字: " sub_choice
        case $sub_choice in
            1) $install_func; pause; break;;
            2) 
                read -p "确定要移除 $app_name 吗？[y/N]: " confirm
                if [[ "$confirm" =~ ^[Yy]$ ]]; then
                    $uninstall_func; pause; break
                fi
                ;;
            0) break;;
            *) warn "无效输入，请重新输入。";;
        esac
    done
}

# ----------------- 业务子菜单 -----------------

handle_warp_submenu() {
    while true; do
        clear
        echo -e "🚀 【 WARP & Usque 出站增强管理 】"
        echo "----------------------------------------------"
        echo -e " 1. CF WARP 官方客户端     $(get_status warp-cli)"
        echo -e " 2. Usque (MASQUE 协议)    $(get_status usque)"
        echo -e " 3. 生成 Xray WG 出站 JSON"
        echo "----------------------------------------------"
        echo -e " 0. 返回主菜单"
        read -p "请选择: " sub_choice
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
        clear
        echo -e "📦 【 GoLang 运行时与自编译生态 】"
        echo "----------------------------------------------"
        echo -e " 1. Go 语言环境 (SDK)      $(get_status go)"
        echo -e " 2. Caddy (插件版编译)     $(get_status caddy)"
        echo -e " 3. DERP 隐身转发节点      $(get_status derper)"
        echo "----------------------------------------------"
        echo -e " 0. 返回主菜单"
        read -p "请选择: " sub_choice
        case $sub_choice in
            1) handle_submenu "Go SDK" install_go uninstall_go;;
            2) handle_submenu "Caddy" install_caddy uninstall_caddy;;
            3) handle_submenu "DERP" install_derper uninstall_derper;;
            0) break;;
            *) warn "无效选择。";;
        esac
    done
}

# ----------------- 主菜单入口 -----------------

show_main_menu() {
    while true; do
        # 刷新哈希表，确保卸载后的命令状态能即时更新
        hash -r 
        clear
        local net_status_text=""
        [[ "$IS_CN_REGION" == "true" ]] && net_status_text="${YELLOW}中国大陆 (镜像加速已开启)${NC}" || net_status_text="${GREEN}海外地区 (直连模式)${NC}"
        
        echo -e "===================================================="
        echo -e "      Debian Optimizer & Service Manager (V2.0)"
        echo -e "      项目架构: 模块化 | 防火墙: nftables"
        echo -e "      当前网络: ${net_status_text}"
        echo -e "===================================================="
        echo -e " 1. 一键系统级基础优化 (内核/BBR/安全/限制)"
        echo -e " 2. 路由转发模式开关   当前: $(get_ip_forward_status)"
        echo "----------------------------------------------------"
        echo -e " 3. Xray Core 节点管理     $(get_status xray)"
        echo -e " 4. Easytier 虚拟组网      $(get_status easytier-core)"
        echo -e " 5. Tailscale 官方组网     $(get_status tailscale)"
        echo -e " 6. WARP & Usque 代理栈    $(get_combined_status warp-cli usque)"
        echo -e " 7. Docker 引擎与编排      $(get_status docker)"
        echo -e " 8. GoLang 编译生态环境    $(get_status go)"
        echo "----------------------------------------------------"
        echo -e " 0. 退出并执行版本提交"
        echo -e "===================================================="
        
        read -p "请输入指令: " choice
        case $choice in
            1) run_base_optimization; pause;;
            2) toggle_ip_forwarding; pause;;
            3) handle_submenu "Xray" install_xray uninstall_xray;;
            4) handle_submenu "Easytier" install_easytier uninstall_easytier;;
            5) handle_submenu "Tailscale" install_tailscale uninstall_tailscale;;
            6) handle_warp_submenu;;
            7) handle_submenu "Docker" install_docker uninstall_docker;;
            8) handle_go_submenu;;
            0) break;;
            *) warn "无效指令: $choice"; sleep 1;;
        esac
    done
}
