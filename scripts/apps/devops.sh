#!/bin/bash
# =========================================================
# 运维与终端增强模块 Fish, Micro, Lego
# =========================================================

# ----------------- Fish Shell 安装与增强 -----------------
install_fish() {
    info "正在安装 Fish Shell 及其生态工具..."
    
    # 1. 安装本体与核心依赖
    safe_apt_install fish fzf fd-find curl git
    
    # 2. 安装 zoxide 与 Starship
    if ! command -v zoxide >/dev/null 2>&1; then
        info "正在通过官方脚本安装 zoxide..."
        local tmp_zoxide="/tmp/zoxide_install.sh"
        if download_with_fallback "$tmp_zoxide" "https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh"; then
            sh "$tmp_zoxide" -y >/dev/null 2>&1 || true
            rm -f "$tmp_zoxide"
        fi
        command -v zoxide >/dev/null 2>&1 || safe_apt_install zoxide
    fi

    # 明确检查两个常见的安装路径：官方脚本默认路径 和 APT 默认路径
    # 只有当两处都不存在可执行文件时，才触发安装流程
    if [[ ! -f "/usr/local/bin/starship" ]] && [[ ! -f "/usr/bin/starship" ]]; then
        info "正在安装 Starship Prompt..."
        
        # 1. 优先尝试使用包管理器安装
        # safe_apt_install 接收单个参数 "starship"
        # 如果成功安装，返回 0，跳过 if 块内的回退逻辑
        # 如果源内无此包或安装失败，返回 1，触发 ! 条件，进入回退逻辑
        if ! safe_apt_install "starship"; then
            warn "APT 源内无 starship 或安装失败，回退到官方脚本安装..."
            
            # 2. 备用方案：下载并执行官方脚本
            local tmp_starship="/tmp/starship_install.sh"
            if download_with_fallback "$tmp_starship" "https://starship.rs/install.sh"; then
                # -y 参数实现非交互式安装，并将标准输出和错误重定向至黑洞保持终端整洁
                sh "$tmp_starship" -y >/dev/null 2>&1
                rm -f "$tmp_starship"
            else
                # 假设你定义过类似于 info 的 err 函数
                die "Starship 官方脚本下载失败，无法完成安装。"
            fi
        fi
    else
        info "Starship 已安装，跳过此步骤。"
    fi

    local sot_user
    sot_user=$(get_sot_user)
    local sot_home
    sot_home=$(eval echo "~$sot_user")
    local all_users=($(get_all_real_users))
    export PATH=$PATH:/usr/local/bin

    # 1. 物理配置真理源 (SOT) 用户的 Fish 环境
    info "正在配置真理源用户 ($sot_user) 的 Fish 物理环境..."
    
    local run_cmd=()
    if [[ "$sot_user" != "root" ]]; then
        run_cmd=("sudo" "-H" "-u" "$sot_user")
    fi

    local fish_conf_dir="$sot_home/.config/fish"
    local functions_dir="$fish_conf_dir/functions"
    local conf_d_dir="$fish_conf_dir/conf.d"

    mkdir -p "$functions_dir" "$conf_d_dir"
    chown -R "$sot_user:$sot_user" "$sot_home/.config" 2>/dev/null || true

    # 1.1 安装 Fisher 插件管理器 (SOT 物理环境)
    if [[ ! -f "$functions_dir/fisher.fish" ]]; then
        info "正在为真理源用户 $sot_user 部署 Fisher..."
        local tmp_fisher="/tmp/fisher.fish"
        if download_with_fallback "$tmp_fisher" "https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish"; then
            "${run_cmd[@]}" fish -c "source $tmp_fisher && fisher install jorgebucaran/fisher" >/dev/null 2>&1 || true
            rm -f "$tmp_fisher"
        fi
    fi
    
    # 1.2 部署核心插件集 (SOT 物理环境)
    info "正在为真理源用户 $sot_user 部署高级插件集..."
    "${run_cmd[@]}" fish -c "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish" >/dev/null 2>&1 || true
    
    # 1.3 配置文件加载 (SOT 物理环境)
    render_template "templates/apps/devops/zoxide.fish" "$conf_d_dir/zoxide.fish"
    render_template "templates/apps/devops/starship.fish" "$conf_d_dir/starship.fish"
    if [[ -f "/usr/local/bin/yazi" ]]; then
        render_template "templates/apps/devops/yazi.fish" "$conf_d_dir/yazi.fish"
    fi
    
    # 1.4 生成全局 Starship 配置文件
    local starship_bin="/usr/local/bin/starship"
    [[ ! -f "$starship_bin" ]] && starship_bin=$(which starship 2>/dev/null || echo "starship")
    
    if command -v "$starship_bin" >/dev/null 2>&1; then
        info "应用 Starship Gruvbox-Rainbow 主题至全局共享目录 /etc/starship.toml ..."
        "$starship_bin" preset gruvbox-rainbow -o "/etc/starship.toml" >/dev/null 2>&1 || true
        chmod 644 /etc/starship.toml
    fi
    
    # 1.5 基础 Abbreviation (SOT 物理环境)
    render_template "templates/apps/devops/abbrs.fish" "$conf_d_dir/abbrs.fish"
    
    # 1.6 复制并同步 Fish 物理配置到全局目录，彻底避免暴露 /root 或用户家目录
    info "正在将 Fish SOT 配置物理同步到全局共享目录 /etc/fish/shared_sot ..."
    rm -rf /etc/fish/shared_sot
    mkdir -p /etc/fish/shared_sot
    cp -rf "$sot_home/.config/fish/"* /etc/fish/shared_sot/
    
    # 设置属主为真理源用户，保证其可更新配置；赋予全部用户读与执行权限
    chown -R "$sot_user:$sot_user" /etc/fish/shared_sot
    chmod -R a+rX /etc/fish/shared_sot
    
    # 2. 为所有真实用户配置软链接 (包含真理源用户本身与 root)
    for u in "${all_users[@]}"; do
        local u_home
        u_home=$(eval echo "~$u")
        [[ ! -d "$u_home" ]] && continue
        
        info "正在将用户 $u 的 Fish/Starship 目录链接至全局共享配置..."
        mkdir -p "$u_home/.config"
        chown "$u:$u" "$u_home/.config" 2>/dev/null || true
        
        rm -rf "$u_home/.config/fish"
        rm -f "$u_home/.config/starship.toml"
        
        ln -sf /etc/fish/shared_sot "$u_home/.config/fish"
        ln -sf /etc/starship.toml "$u_home/.config/starship.toml"
        
        chown -h "$u:$u" "$u_home/.config/fish" "$u_home/.config/starship.toml" 2>/dev/null || true
    done

    # 3. 设置默认 Shell 为 Fish (对包括 root 在内的所有真实用户生效)
    for u in "${all_users[@]}"; do
        info "正在将用户 $u 的默认 Shell 设置为 Fish..."
        local fish_path=$(which fish 2>/dev/null || echo "/usr/bin/fish")
        chsh -s "$fish_path" "$u" 2>/dev/null || true
    done
    
    success "Fish Shell 现代化 SOT 环境部署完成。"
}

uninstall_fish() {
    info "正在移除 Fish Shell 及其生态工具..."
    local all_users=($(get_all_real_users))
    
    # 1. 恢复所有真实用户（包括 root）的默认 Shell 为 bash
    for user in "${all_users[@]}"; do
        chsh -s /bin/bash "$user" 2>/dev/null || true
    done
    
    # 2. 物理清除包与二进制
    apt-get purge -yq fish zoxide
    rm -f /usr/local/bin/starship
    
    # 3. 彻底清除每个用户的 Fish 和 Starship 软链与配置，以及全局共享文件
    for user in "${all_users[@]}"; do
        local user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/fish"
        rm -f "$user_home/.config/starship.toml"
    done
    rm -rf /etc/fish/shared_sot
    rm -f /etc/starship.toml
    
    success "Fish 及其配置已彻底清理。"
}

# ----------------- Micro 编辑器安装 -----------------
install_micro() {
    info "正在安装 Micro 编辑器 最新二进制版..."
    # 补齐 xclip（剪贴板）、linter依赖（shellcheck, yamllint）以及 MicroOmni 依赖（fzf, ripgrep, bat）
    safe_apt_install xclip shellcheck yamllint fzf ripgrep bat
    
    # 解决 Debian 下 bat 包安装后可执行文件为 batcat 的冲突，为其在 /usr/local/bin 下创建 bat 软链接
    if command -v batcat >/dev/null 2>&1 && [[ ! -f "/usr/local/bin/bat" ]]; then
        ln -sf "$(which batcat)" /usr/local/bin/bat
    fi

    # 明确检查两个常见的安装路径：官方脚本移动的路径 和 APT 默认路径
    if [[ ! -f "/usr/local/bin/micro" ]] && [[ ! -f "/usr/bin/micro" ]]; then
        info "正在安装 Micro 文本编辑器..."
        
        # 1. 优先尝试包管理器安装
        if ! safe_apt_install "micro"; then
            warn "APT 源内无 micro 或安装失败，回退到官方脚本安装..."
            
            # 2. 备用方案：下载官方脚本
            local tmp_micro="/tmp/get_micro.sh"
            if download_with_fallback "$tmp_micro" "https://getmic.ro"; then
                # 使用子shell (...) 执行 cd 操作，确保不会改变主脚本的当前工作目录
                # 静默执行并在成功后移动二进制文件
                if ( cd /tmp && bash "$tmp_micro" >/dev/null 2>&1 && mv micro /usr/local/bin/ ); then
                    rm -f "$tmp_micro"
                else
                    die "Micro 脚本执行或移动文件失败！"
                fi
            else
                die "Micro 官方脚本下载失败，无法完成安装！"
            fi
        fi
    else
        info "Micro 已安装，跳过此步骤。"
    fi

    # 确定真理源 (SOT) 账户与物理存储
    local sot_user
    sot_user=$(get_sot_user)
    local sot_home
    sot_home=$(eval echo "~$sot_user")
    
    # 1. 为真理源用户物理生成配置目录与设置，绑定自定义快捷键和初始化逻辑
    mkdir -p "$sot_home/.config/micro"
    render_template "templates/apps/devops/micro_settings.json" "$sot_home/.config/micro/settings.json"
    render_template "templates/apps/devops/micro_bindings.json" "$sot_home/.config/micro/bindings.json"
    render_template "templates/apps/devops/micro_init.lua" "$sot_home/.config/micro/init.lua"

    # 2. 下载并部署所需插件 (自动适配国内外镜像源)
    info "正在部署 Micro 插件集..."
    local plug_dir="$sot_home/.config/micro/plug"
    mkdir -p "$plug_dir"
    
    # 定义插件列表与对应的 GitHub 仓库地址
    local plugins=(
        "MicroOmni|https://github.com/Neko-Box-Coder/MicroOmni"
        "gutter_message|https://github.com/usfbih8u/micro-gutter-message"
        "snippets|https://github.com/micro-editor/updated-plugins.git"
        "gitStatus|https://github.com/Neko-Box-Coder/git-status"
    )
    
    for item in "${plugins[@]}"; do
        local name="${item%%|*}"
        local repo="${item#*|}"
        info "正在部署插件: $name ..."
        
        if [[ "$name" == "snippets" ]]; then
            # snippets 插件已并入 updated-plugins 单体仓库，需拉取后提取其子目录部署
            local tmp_dir="/tmp/micro_updated_plugins"
            rm -rf "$tmp_dir"
            if git_clone_with_fallback "$tmp_dir" "$repo" --depth=1; then
                mkdir -p "$plug_dir/snippets"
                cp -rf "$tmp_dir/micro-snippets-plugin/"* "$plug_dir/snippets/"
                rm -rf "$tmp_dir"
            else
                warn "插件 snippets 部署失败，后续可能需要手动安装。"
            fi
        else
            # 使用项目的高可用 git 克隆函数进行防封锁克隆
            git_clone_with_fallback "$plug_dir/$name" "$repo" --depth=1 || warn "插件 $name 部署失败，后续可能需要手动安装。"
        fi
    done

    # 修正真理源配置的属主权限 (包含新下载的插件目录)，确保普通用户可正常读写与执行
    chown -R "$sot_user:$sot_user" "$sot_home/.config/micro" 2>/dev/null || true

    # 放开真理源配置的可读与可执行权限供其他用户同步软链
    chmod o+rx "$sot_home" 2>/dev/null || true
    chmod o+rx "$sot_home/.config" 2>/dev/null || true
    chmod -R o+rX "$sot_home/.config/micro" 2>/dev/null || true

    # 3. 为所有其他真实用户配置软链接
    local all_users=($(get_all_real_users))
    for u in "${all_users[@]}"; do
        if [[ "$u" != "$sot_user" ]]; then
            local u_home
            u_home=$(eval echo "~$u")
            if [[ -d "$u_home" ]]; then
                mkdir -p "$u_home/.config"
                chown "$u:$u" "$u_home/.config" 2>/dev/null || true
                rm -rf "$u_home/.config/micro"
                ln -sf "$sot_home/.config/micro" "$u_home/.config/micro"
                chown -h "$u:$u" "$u_home/.config/micro" 2>/dev/null || true
            fi
        fi
    done

    # 4. 注册全局环境变量与 Fish 配置
    local profile_file="/etc/profile.d/micro_env.sh"
    render_template "templates/apps/devops/micro_env.sh" "$profile_file"
    chmod +x "$profile_file"
    set_system_env "MICRO_TRUECOLOR" "1"
    set_system_env "EDITOR" "micro"
    set_system_env "VISUAL" "micro"
    update_fish_env "MICRO_TRUECOLOR" "1"
    update_fish_env "EDITOR" "micro"
    update_fish_env "VISUAL" "micro"
    
    # 5. 替代项优先级覆盖绑定
    local final_micro_bin=""
    if [[ -f "/usr/local/bin/micro" ]]; then
        final_micro_bin="/usr/local/bin/micro"
    elif [[ -f "/usr/bin/micro" ]]; then
        final_micro_bin="/usr/bin/micro"
    fi

    if [[ -n "$final_micro_bin" ]]; then
        update-alternatives --install /usr/bin/editor editor "$final_micro_bin" 100 || true
        update-alternatives --set editor "$final_micro_bin" || true
    else
        warn "未能在系统路径中定位到 micro，跳过编辑器替代项配置。"
    fi
    success "Micro 进阶优化配置完成。"
}

uninstall_micro() {
    info "正在移除 Micro..."
    # 移除所有可能的编辑器替代项绑定
    update-alternatives --remove editor /usr/local/bin/micro >/dev/null 2>&1 || true
    update-alternatives --remove editor /usr/bin/micro >/dev/null 2>&1 || true

    # 物理清理二进制文件与包
    rm -f /usr/local/bin/micro
    rm -f /usr/local/bin/bat
    apt-get purge -yq micro xclip
    
    # 清理所有用户的配置文件
    local all_users=($(get_all_real_users))
    for user in "${all_users[@]}"; do
        local user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/micro"
    done
    
    rm -f /etc/profile.d/micro_env.sh
    remove_system_env "MICRO_TRUECOLOR"
    remove_system_env "EDITOR"
    remove_system_env "VISUAL"
    remove_fish_env "MICRO_TRUECOLOR"
    remove_fish_env "EDITOR"
    remove_fish_env "VISUAL"
    success "Micro 已彻底移除。"
}

# ----------------- Lego 证书工具安装 -----------------
install_lego() {
    info "正在部署 Lego 自动化证书管理工具..."
    
    local arch=""
    case "$(uname -m)" in
        x86_64) arch="amd64" ;;
        aarch64) arch="arm64" ;;
        *) err "不支持的架构: $(uname -m)"; return 1 ;;
    esac

    info "正在获取 Lego 最新版本信息..."
    local latest_version
    latest_version=$(get_latest_github_release "go-acme/lego")
    if [[ -z "$latest_version" ]]; then
        latest_version="v5.2.2"
        warn "获取最新版本失败，将尝试安装稳定版: $latest_version"
    fi

    if [[ ! "$latest_version" =~ ^v ]]; then
        latest_version="v${latest_version}"
    fi

    local download_url="https://github.com/go-acme/lego/releases/download/${latest_version}/lego_${latest_version}_linux_${arch}.tar.gz"
    local tmp_file="/tmp/lego.tar.gz"
    
    download_with_fallback "$tmp_file" "$download_url" || return 1

    local tmp_dir="/tmp/lego_extract"
    mkdir -p "$tmp_dir"
    tar -xzf "$tmp_file" -C "$tmp_dir" || { err "解压 Lego 失败。"; rm -rf "$tmp_dir" "$tmp_file"; return 1; }
    
    mv "$tmp_dir/lego" /usr/local/bin/lego
    chmod +x /usr/local/bin/lego
    rm -rf "$tmp_dir" "$tmp_file"

    # 初始化配置与工作目录
    mkdir -p /etc/lego/envs /var/lib/lego/certificates

    # 通过模板引擎部署自动更新相关的脚本与 Systemd 任务
    info "部署自动续期脚本与定时任务..."
    render_template "templates/apps/lego/debopti-lego-renew.sh" "/usr/local/bin/debopti-lego-renew.sh"
    render_template "templates/apps/lego/debopti-lego-hook.sh" "/usr/local/bin/debopti-lego-hook.sh"
    chmod +x /usr/local/bin/debopti-lego-renew.sh /usr/local/bin/debopti-lego-hook.sh

    render_template "templates/apps/lego/debopti-lego-renew.service" "/etc/systemd/system/debopti-lego-renew.service"
    render_template "templates/apps/lego/debopti-lego-renew.timer" "/etc/systemd/system/debopti-lego-renew.timer"

    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable --now debopti-lego-renew.timer >/dev/null 2>&1 || true

    success "Lego 部署及自动续期定时任务配置完成。"
}

uninstall_lego() {
    info "正在移除 Lego 及其自动化托管资源..."
    
    # 停止并禁用定时器
    systemctl disable --now debopti-lego-renew.timer >/dev/null 2>&1 || true
    systemctl stop debopti-lego-renew.service >/dev/null 2>&1 || true
    
    # 物理清理二进制及脚本
    rm -f /usr/local/bin/lego
    rm -f /usr/local/bin/debopti-lego-renew.sh
    rm -f /usr/local/bin/debopti-lego-hook.sh
    rm -f /etc/systemd/system/debopti-lego-renew.service
    rm -f /etc/systemd/system/debopti-lego-renew.timer
    
    systemctl daemon-reload >/dev/null 2>&1 || true

    # 深度清理配置与证书存放目录
    rm -rf /etc/lego /var/lib/lego
    
    success "Lego 及其自动化组件已彻底从系统中清空。"
}

# ----------------- Lego 证书管理子系统 -----------------

handle_lego_submenu() {
    if ! command -v lego >/dev/null 2>&1 && [[ ! -f "/usr/local/bin/lego" ]]; then
        # Lego 未安装，引导用户安装
        while true; do
            ui_draw_header "Lego 证书工具管理" "Main > DevOps > Lego"
            echo -e " 状态: ${DIM}未部署${NC}"
            ui_draw_sep
            ui_draw_item "1" "✨ 安装 Lego 证书工具"
            ui_draw_sep
            ui_draw_item "0" "🔙 返回上级菜单"
            echo ""
            read -p " >>> 选择: " sub_choice
            case $sub_choice in
                1) install_lego; pause;;
                0) break;;
                *) warn "无效选择。";;
            esac
        done
        return 0
    fi

    # Lego 已安装，展示配置看板
    while true; do
        ui_draw_header "Lego 自动化证书管理" "Main > DevOps > Lego"
        
        local env_dir="/etc/lego/envs"
        local env_files=()
        if [[ -d "$env_dir" ]]; then
            shopt -s nullglob
            env_files=("$env_dir"/*.env)
            shopt -u nullglob
        fi
        
        echo -e " ${BOLD}当前托管的证书列表:${NC}"
        echo -e " ------------------------------------------------------------"
        
        local count=${#env_files[@]}
        if [[ $count -eq 0 ]]; then
            echo -e " ${DIM}(无托管域名，请选择 [A] 添加新域名)${NC}"
        else
            for ((i=0; i<count; i++)); do
                local env_file="${env_files[i]}"
                local domain_config
                domain_config=$( (
                    source "$env_file" 2>/dev/null
                    echo "${DEBOPTI_DOMAINS:-}|${DEBOPTI_EMAIL:-}|${DEBOPTI_PROVIDER:-}|${DEBOPTI_AUTO_RENEW:-}|${DEBOPTI_FERRON_PUSH:-}"
                ) )
                
                IFS='|' read -r domains email provider auto_renew ferron_push <<< "$domain_config"
                local primary_domain="${domains%%,*}"
                
                # 读取证书状态
                local cert_path="/var/lib/lego/certificates/${primary_domain}.crt"
                local status_text=""
                local last_update=""
                if [[ -f "$cert_path" ]]; then
                    local end_date_str
                    end_date_str=$(openssl x509 -enddate -noout -in "$cert_path" | cut -d= -f2)
                    local end_epoch
                    end_epoch=$(date -d "$end_date_str" +%s 2>/dev/null || echo 0)
                    local now_epoch
                    now_epoch=$(date +%s)
                    local days_left=$(( (end_epoch - now_epoch) / 86400 ))
                    
                    if [[ $days_left -lt 0 ]]; then
                        status_text="${RED}已过期 ($(( -days_left ))天前)${NC}"
                    elif [[ $days_left -le 30 ]]; then
                        status_text="${YELLOW}即将到期 (${days_left}天后)${NC}"
                    else
                        status_text="${GREEN}有效 (${days_left}天后)${NC}"
                    fi
                    last_update=$(date -d "$(stat -c %y "$cert_path")" "+%Y-%m-%d %H:%M:%S" 2>/dev/null || echo "未知")
                else
                    status_text="${YELLOW}未申请 (待手动首次申请)${NC}"
                    last_update="N/A"
                fi
                
                # 检查 Ferron 状态
                local ferron_push_text="关闭"
                if [[ "$ferron_push" == "true" ]]; then
                    if [[ -d "/etc/ferron" ]] || command -v ferron >/dev/null 2>&1; then
                        ferron_push_text="${GREEN}开启${NC}"
                    else
                        ferron_push_text="${DIM}开启 (未安装 Ferron)${NC}"
                    fi
                else
                    ferron_push_text="${DIM}关闭${NC}"
                fi
                
                local renew_text
                [[ "$auto_renew" == "true" ]] && renew_text="${GREEN}开启${NC}" || renew_text="${DIM}关闭${NC}"
                
                echo -e "  [${BOLD}$((i+1))${NC}] ${CYAN}${primary_domain}${NC}"
                echo -e "      ${DIM}├─ 域名:${NC} $domains"
                echo -e "      ${DIM}├─ 状态:${NC} $status_text | ${DIM}更新时间:${NC} $last_update"
                echo -e "      ${DIM}└─ 自动更新:${NC} $renew_text | ${DIM}Ferron 推送:${NC} $ferron_push_text"
            done
        fi
        
        echo -e " ------------------------------------------------------------"
        ui_draw_item "A" "✨ 添加新域名证书管理"
        ui_draw_item "U" "🗑️ 卸载 Lego 客户端"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回上级菜单"
        echo ""
        
        read -p " >>> 选择: " choice
        case $choice in
            [aA])
                handle_lego_add_domain
                ;;
            [uU])
                uninstall_lego; pause; break
                ;;
            0)
                break
                ;;
            *)
                if [[ "$choice" =~ ^[0-9]+$ ]] && (( choice >= 1 && choice <= count )); then
                    local selected_env="${env_files[choice-1]}"
                    handle_lego_domain_detail "$selected_env"
                else
                    warn "无效选择。"
                    sleep 1
                fi
                ;;
        esac
    done
}

handle_lego_add_domain() {
    ui_draw_header "添加新域名证书管理" "Main > Lego > Add"
    
    echo -e " ${BOLD}请输入新证书信息 (输入 0 可随时退出):${NC}"
    
    local primary_domain=""
    while [[ -z "$primary_domain" ]]; do
        read -p " 1. 请输入主域名 (例如: example.com): " primary_domain
        [[ "$primary_domain" == "0" ]] && return 0
        if [[ -z "$primary_domain" ]]; then
            warn "主域名不能为空！"
        fi
    done
    
    local sub_domains=""
    read -p " 2. 请输入备用域名 (例如: *.example.com，多个用逗号隔开，可选): " sub_domains
    [[ "$sub_domains" == "0" ]] && return 0
    
    local email=""
    while [[ -z "$email" ]]; do
        read -p " 3. 请输入联系邮箱 (用于 Let's Encrypt 过期通知): " email
        [[ "$email" == "0" ]] && return 0
        if [[ -z "$email" ]]; then
            warn "联系邮箱不能为空！"
        fi
    done
    
    local cf_token=""
    while [[ -z "$cf_token" ]]; do
        read -p " 4. 请输入 Cloudflare DNS API Token: " cf_token
        [[ "$cf_token" == "0" ]] && return 0
        if [[ -z "$cf_token" ]]; then
            warn "API Token 不能为空！"
        fi
    done
    
    # 构造完整域名参数
    local domains="$primary_domain"
    if [[ -n "$sub_domains" ]]; then
        domains="${primary_domain},${sub_domains}"
    fi
    
    local env_file="/etc/lego/envs/${primary_domain}.env"
    mkdir -p "/etc/lego/envs"
    
    render_template "templates/apps/lego/lego.env" "$env_file" \
        "CF_TOKEN=$cf_token" \
        "DOMAINS=$domains" \
        "EMAIL=$email" \
        "PROVIDER=cloudflare" \
        "AUTO_RENEW=true" \
        "FERRON_PUSH=false"
        
    success "域名配置已保存到 $env_file"
    echo ""
    
    # 打印首次手动命令模板
    echo -e " ${GREEN}✔ 配置已成功生成！${NC}"
    echo -e " 为了完成首次证书申请，请 ${BOLD}复制并在命令行执行以下命令${NC}："
    echo -e " ------------------------------------------------------------"
    echo -e " ${YELLOW}CLOUDFLARE_DNS_API_TOKEN=\"${cf_token}\" \\"
    echo -e " lego --email=\"${email}\" \\"
    echo -e "      --dns=\"cloudflare\" \\"
    IFS=',' read -ra ADDR <<< "$domains"
    for d in "${ADDR[@]}"; do
        echo -e "      --domains=\"$d\" \\"
    done
    echo -e "      --path=\"/var/lib/lego\" \\"
    echo -e "      --accept-tos \\"
    echo -e "      run${NC}"
    echo -e " ------------------------------------------------------------"
    
    pause
}

handle_lego_domain_detail() {
    local env_file=$1
    while true; do
        # 实时载入配置
        local domain_config
        domain_config=$( (
            source "$env_file" 2>/dev/null
            echo "${DEBOPTI_DOMAINS:-}|${DEBOPTI_EMAIL:-}|${DEBOPTI_PROVIDER:-}|${DEBOPTI_AUTO_RENEW:-}|${DEBOPTI_FERRON_PUSH:-}"
        ) )
        
        IFS='|' read -r domains email provider auto_renew ferron_push <<< "$domain_config"
        local primary_domain="${domains%%,*}"
        
        # 读取证书状态
        local cert_path="/var/lib/lego/certificates/${primary_domain}.crt"
        local status_text=""
        local last_update=""
        if [[ -f "$cert_path" ]]; then
            local end_date_str
            end_date_str=$(openssl x509 -enddate -noout -in "$cert_path" | cut -d= -f2)
            local end_epoch
            end_epoch=$(date -d "$end_date_str" +%s 2>/dev/null || echo 0)
            local now_epoch
            now_epoch=$(date +%s)
            local days_left=$(( (end_epoch - now_epoch) / 86400 ))
            
            if [[ $days_left -lt 0 ]]; then
                status_text="${RED}已过期 ($(( -days_left ))天前)${NC}"
            elif [[ $days_left -le 30 ]]; then
                status_text="${YELLOW}即将到期 (${days_left}天后)${NC}"
            else
                status_text="${GREEN}有效 (${days_left}天后)${NC}"
            fi
            last_update=$(date -d "$(stat -c %y "$cert_path")" "+%Y-%m-%d %H:%M:%S" 2>/dev/null || echo "未知")
        else
            status_text="${YELLOW}未申请 (待手动首次申请)${NC}"
            last_update="N/A"
        fi
        
        ui_draw_header "证书管理: $primary_domain" "Main > Lego > Detail"
        echo -e " ${BOLD}配置与状态:${NC}"
        echo -e "  - 域名列表: $domains"
        echo -e "  - 联系邮箱: $email"
        echo -e "  - DNS 驱动: $provider"
        echo -e "  - 证书状态: $status_text"
        echo -e "  - 最后更新: $last_update"
        ui_draw_sep
        
        local renew_toggle_text
        [[ "$auto_renew" == "true" ]] && renew_toggle_text="${GREEN}开启${NC}" || renew_toggle_text="${DIM}关闭${NC}"
        ui_draw_item "1" "🔄 切换自动更新 (当前: $renew_toggle_text)"
        
        # 仅在系统已部署 Ferron 时提供推送开关
        local show_ferron_option=false
        if [[ -d "/etc/ferron" ]] || command -v ferron >/dev/null 2>&1; then
            show_ferron_option=true
            local push_toggle_text
            [[ "$ferron_push" == "true" ]] && push_toggle_text="${GREEN}开启${NC}" || push_toggle_text="${DIM}关闭${NC}"
            ui_draw_item "2" "🚀 切换 Ferron 推送 (当前: $push_toggle_text)"
        fi
        
        ui_draw_item "3" "📝 编辑环境配置文件 (.env)"
        ui_draw_item "4" "📋 查看首次申请命令模板"
        ui_draw_item "5" "⚡ 立即测试手动续期/申请"
        ui_draw_item "6" "🗑️ 移除此域名证书管理 (保留已申请证书)"
        ui_draw_sep
        ui_draw_item "0" "🔙 返回列表"
        echo ""
        
        read -p " >>> 选择: " detail_choice
        case $detail_choice in
            1)
                if [[ "$auto_renew" == "true" ]]; then
                    set_conf_value "$env_file" "export DEBOPTI_AUTO_RENEW" "\"false\""
                else
                    set_conf_value "$env_file" "export DEBOPTI_AUTO_RENEW" "\"true\""
                fi
                ;;
            2)
                if [[ "$show_ferron_option" == "true" ]]; then
                    if [[ "$ferron_push" == "true" ]]; then
                        set_conf_value "$env_file" "export DEBOPTI_FERRON_PUSH" "\"false\""
                    else
                        set_conf_value "$env_file" "export DEBOPTI_FERRON_PUSH" "\"true\""
                        # 开启推送时，若证书已存在，则立即触发一次推送重载
                        if [[ -f "/var/lib/lego/certificates/${primary_domain}.crt" ]]; then
                            info "正在执行首次证书推送并重载 Ferron..."
                            /usr/local/bin/debopti-lego-hook.sh "$primary_domain" || true
                            sleep 1
                        fi
                    fi
                else
                    warn "未检测到 Ferron Web 服务器，该选项不可用。"
                    sleep 1
                fi
                ;;
            3)
                local editor="nano"
                command -v micro >/dev/null 2>&1 && editor="micro"
                $editor "$env_file"
                ;;
            4)
                ui_draw_header "命令模板: $primary_domain" "Main > Lego > Template"
                local token
                token=$(grep -E 'CLOUDFLARE_DNS_API_TOKEN=' "$env_file" | cut -d'"' -f2 || echo "你的_Cloudflare_API_Token")
                echo -e " 为了完成首次证书申请，请复制并在终端运行以下命令："
                echo -e " ------------------------------------------------------------"
                echo -e " ${YELLOW}CLOUDFLARE_DNS_API_TOKEN=\"${token}\" \\"
                echo -e " lego --email=\"${email}\" \\"
                echo -e "      --dns=\"${provider}\" \\"
                IFS=',' read -ra ADDR <<< "$domains"
                for d in "${ADDR[@]}"; do
                    echo -e "      --domains=\"$d\" \\"
                done
                echo -e "      --path=\"/var/lib/lego\" \\"
                echo -e "      --accept-tos \\"
                echo -e "      run${NC}"
                echo -e " ------------------------------------------------------------"
                pause
                ;;
            5)
                info "正在启动 Lego 手动申请/续期测试..."
                (
                    source "$env_file"
                    domain_args=""
                    IFS=',' read -ra ADDR <<< "$DEBOPTI_DOMAINS"
                    for d in "${ADDR[@]}"; do
                        domain_args="$domain_args --domains=$d"
                    done
                    
                    # 证书不存在时运行 run，否则运行 renew
                    if [[ ! -f "/var/lib/lego/certificates/${primary_domain}.crt" ]]; then
                        /usr/local/bin/lego --email="$DEBOPTI_EMAIL" \
                                       --dns="$DEBOPTI_PROVIDER" \
                                       $domain_args \
                                       --path="/var/lib/lego" \
                                       --accept-tos \
                                       run
                    else
                        /usr/local/bin/lego --email="$DEBOPTI_EMAIL" \
                                       --dns="$DEBOPTI_PROVIDER" \
                                       $domain_args \
                                       --path="/var/lib/lego" \
                                       --accept-tos \
                                       renew --days 30 \
                                       --renew-hook "/usr/local/bin/debopti-lego-hook.sh $primary_domain"
                    fi
                )
                pause
                ;;
            6)
                read -p " 确认要移除此域名配置吗？此操作不会删除已申请的证书。(y/n): " confirm
                if [[ "$confirm" == "y" || "$confirm" == "Y" ]]; then
                    rm -f "$env_file"
                    success "配置已移除。"
                    sleep 1
                    break
                fi
                ;;
            0)
                break
                ;;
        esac
    done
}

# ----------------- Yazi 文件管理器安装 -----------------
install_yazi() {
    info "正在安装 Yazi 极速终端文件管理器..."
    safe_apt_install unzip file jq p7zip-full

    # 1. 获取 Yazi 最新 Release 版本
    local latest_version
    latest_version=$(get_latest_github_release "sxyazi/yazi")
    if [[ ! "$latest_version" =~ ^v?[0-9] ]]; then
        latest_version="v0.4.0" # 兜底机制
        warn "无法从 GitHub 获取最新版本，使用默认兜底版本: $latest_version"
    fi
    info "Yazi 目标安装版本: $latest_version"

    # 2. 判断系统架构并下载对应二进制文件
    local arch
    arch=$(uname -m)
    local asset_name=""
    if [[ "$arch" == "x86_64" ]]; then
        asset_name="yazi-x86_64-unknown-linux-musl.zip"
    elif [[ "$arch" == "aarch64" ]]; then
        asset_name="yazi-aarch64-unknown-linux-musl.zip"
    else
        die "不支持的系统架构: $arch"
    fi

    local tmp_zip="/tmp/yazi_${latest_version}.zip"
    local extract_dir="/tmp/yazi_extracted_${latest_version}"
    rm -f "$tmp_zip"
    rm -rf "$extract_dir"

    local download_url="https://github.com/sxyazi/yazi/releases/download/${latest_version}/${asset_name}"
    if download_with_fallback "$tmp_zip" "$download_url"; then
        mkdir -p "$extract_dir"
        if unzip -q -o "$tmp_zip" -d "$extract_dir"; then
            # 动态检索解压目录下的二进制可执行文件 yazi 与 ya，并移动到全局路径
            local bin_yazi
            bin_yazi=$(find "$extract_dir" -type f -name "yazi" -executable | head -n1)
            local bin_ya
            bin_ya=$(find "$extract_dir" -type f -name "ya" -executable | head -n1)

            if [[ -n "$bin_yazi" && -n "$bin_ya" ]]; then
                mv -f "$bin_yazi" /usr/local/bin/yazi
                mv -f "$bin_ya" /usr/local/bin/ya
                chmod +x /usr/local/bin/yazi /usr/local/bin/ya
            else
                die "解压包中未找到有效的 yazi 或 ya 可执行二进制文件！"
            fi
        else
            die "解压 Yazi 压缩包失败！"
        fi
        rm -f "$tmp_zip"
        rm -rf "$extract_dir"
    else
        die "下载 Yazi 二进制安装包失败！"
    fi

    # 3. 确定真理源 (SOT) 账户与物理存储
    local sot_user
    sot_user=$(get_sot_user)
    local sot_home
    sot_home=$(eval echo "~$sot_user")
    
    # 4. 为真理源用户物理生成配置目录并渲染配置模板
    local sot_yazi_conf="$sot_home/.config/yazi"
    mkdir -p "$sot_yazi_conf"
    render_template "templates/apps/devops/yazi.toml" "$sot_yazi_conf/yazi.toml"
    render_template "templates/apps/devops/yazi_keymap.toml" "$sot_yazi_conf/keymap.toml"
    
    # 5. 全局及多用户注册 Fish Shell wrapper
    if [[ -d "/etc/fish/conf.d" ]]; then
        render_template "templates/apps/devops/yazi.fish" "/etc/fish/conf.d/yazi.fish"
    fi
    local sot_fish_conf="$sot_home/.config/fish/conf.d"
    if [[ -d "$sot_home/.config/fish" ]]; then
        mkdir -p "$sot_fish_conf"
        render_template "templates/apps/devops/yazi.fish" "$sot_fish_conf/yazi.fish"
    fi

    # 5.3 全局注册 Bash/Zsh wrapper 脚本并注入到 /etc/bash.bashrc
    local wrapper_profile="/etc/profile.d/yazi_wrapper.sh"
    render_template "templates/apps/devops/yazi_wrapper.sh" "$wrapper_profile"
    chmod +x "$wrapper_profile"

    if ! grep -q "# Yazi CWD Sync Wrapper" /etc/bash.bashrc; then
        cat >> /etc/bash.bashrc << 'EOF'

# Yazi CWD Sync Wrapper
if [ -f /etc/profile.d/yazi_wrapper.sh ]; then
    . /etc/profile.d/yazi_wrapper.sh
fi
EOF
    fi

    # 修正真理源配置所有者
    chown -R "$sot_user:$sot_user" "$sot_yazi_conf" 2>/dev/null || true
    [[ -d "$sot_home/.config/fish" ]] && chown -R "$sot_user:$sot_user" "$sot_home/.config/fish" 2>/dev/null || true

    # 5.5 自动为真理源用户部署常用官方插件 (允许网络超时/失败退出，不阻塞主流程)
    if [[ -x "/usr/local/bin/ya" ]]; then
        info "正在为真理源用户安装常用 Yazi 插件 (git, chmod, max-preview)..."
        local run_cmd=()
        if [[ "$sot_user" != "root" ]]; then
            run_cmd=("sudo" "-H" "-u" "$sot_user")
        fi
        "${run_cmd[@]}" /usr/local/bin/ya pkg add yazi-rs/plugins:git >/dev/null 2>&1 || true
        "${run_cmd[@]}" /usr/local/bin/ya pkg add yazi-rs/plugins:chmod >/dev/null 2>&1 || true
        "${run_cmd[@]}" /usr/local/bin/ya pkg add yazi-rs/plugins:max-preview >/dev/null 2>&1 || true
        # 重新修正可能由 sudo 创建的文件属主权限
        chown -R "$sot_user:$sot_user" "$sot_yazi_conf" 2>/dev/null || true
    fi

    # 开放真理源配置目录权限以供其他用户共享软链接
    chmod o+rx "$sot_home" 2>/dev/null || true
    chmod o+rx "$sot_home/.config" 2>/dev/null || true
    chmod -R o+rX "$sot_yazi_conf" 2>/dev/null || true

    # 6. 为所有其他真实用户配置软链接与 Fish wrapper
    local all_users=($(get_all_real_users))
    for u in "${all_users[@]}"; do
        if [[ "$u" != "$sot_user" ]]; then
            local u_home
            u_home=$(eval echo "~$u")
            if [[ -d "$u_home" ]]; then
                mkdir -p "$u_home/.config"
                chown "$u:$u" "$u_home/.config" 2>/dev/null || true
                rm -rf "$u_home/.config/yazi"
                ln -sf "$sot_yazi_conf" "$u_home/.config/yazi"
                chown -h "$u:$u" "$u_home/.config/yazi" 2>/dev/null || true

                # 为其他用户的 Fish Shell 部署目录同步函数
                if [[ -d "$u_home/.config/fish" ]]; then
                    mkdir -p "$u_home/.config/fish/conf.d"
                    render_template "templates/apps/devops/yazi.fish" "$u_home/.config/fish/conf.d/yazi.fish"
                    chown -R "$u:$u" "$u_home/.config/fish" 2>/dev/null || true
                fi
            fi
        fi
    done

    success "Yazi 文件管理器安装配置完成。您可以在终端中直接输入 'y' 启动以获得自动 CWD 同步支持。"
}

uninstall_yazi() {
    info "正在移除 Yazi 文件管理器..."
    
    # 1. 物理清理二进制程序与全局包装器
    rm -f /usr/local/bin/yazi
    rm -f /usr/local/bin/ya
    rm -f /etc/profile.d/yazi_wrapper.sh
    rm -f /etc/fish/conf.d/yazi.fish

    # 清理 /etc/bash.bashrc 中的注入
    if [[ -f /etc/bash.bashrc ]]; then
        sed -i '/# Yazi CWD Sync Wrapper/,+3d' /etc/bash.bashrc
    fi

    # 2. 清理所有用户的配置文件
    local all_users=($(get_all_real_users))
    for user in "${all_users[@]}"; do
        local user_home
        user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/yazi"
        rm -f "$user_home/.config/fish/conf.d/yazi.fish"
    done

    success "Yazi 已彻底从系统移除。"
}
