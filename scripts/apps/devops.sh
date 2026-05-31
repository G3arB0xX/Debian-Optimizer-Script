#!/bin/bash
# =========================================================
# 运维与终端增强模块 Fish, Micro, Acme.sh
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
    
    # 1.4 应用 Starship 主题 (SOT 物理环境)
    local starship_bin="/usr/local/bin/starship"
    [[ ! -f "$starship_bin" ]] && starship_bin=$(which starship 2>/dev/null || echo "starship")
    
    if command -v "$starship_bin" >/dev/null 2>&1; then
        info "应用 Starship Gruvbox-Rainbow 主题..."
        "${run_cmd[@]}" "$starship_bin" preset gruvbox-rainbow -o "$sot_home/.config/starship.toml" >/dev/null 2>&1 || true
    fi
    
    # 1.5 基础 Abbreviation (SOT 物理环境)
    render_template "templates/apps/devops/abbrs.fish" "$conf_d_dir/abbrs.fish"
    
    # 1.6 终极权限修复与可读性开放 (让其他真实用户可继承)
    chown -R "$sot_user:$sot_user" "$sot_home/.config" 2>/dev/null || true
    chmod o+rx "$sot_home" 2>/dev/null || true
    chmod o+rx "$sot_home/.config" 2>/dev/null || true
    chmod -R o+rX "$sot_home/.config/fish" 2>/dev/null || true
    [[ -f "$sot_home/.config/starship.toml" ]] && chmod o+r "$sot_home/.config/starship.toml" 2>/dev/null || true

    # 2. 为所有其他真实用户配置软链接直连真理源
    for u in "${all_users[@]}"; do
        if [[ "$u" != "$sot_user" ]]; then
            local u_home
            u_home=$(eval echo "~$u")
            [[ ! -d "$u_home" ]] && continue
            
            info "正在将用户 $u 的 Fish/Starship 目录链接至真理源..."
            mkdir -p "$u_home/.config"
            chown "$u:$u" "$u_home/.config" 2>/dev/null || true
            
            # 清理旧的目录或链接，然后创建软链
            rm -rf "$u_home/.config/fish"
            rm -f "$u_home/.config/starship.toml"
            
            ln -sf "$sot_home/.config/fish" "$u_home/.config/fish"
            ln -sf "$sot_home/.config/starship.toml" "$u_home/.config/starship.toml"
            
            chown -h "$u:$u" "$u_home/.config/fish" "$u_home/.config/starship.toml" 2>/dev/null || true
        fi
    done

    # 3. 设置默认 Shell (对所有普通用户生效，root 保持 bash)
    for u in "${all_users[@]}"; do
        if [[ "$u" != "root" ]]; then
            info "正在将用户 $u 的默认 Shell 设置为 Fish..."
            local fish_path=$(which fish 2>/dev/null || echo "/usr/bin/fish")
            chsh -s "$fish_path" "$u" 2>/dev/null || true
        fi
    done
    
    success "Fish Shell 现代化 SOT 环境部署完成。"
}

uninstall_fish() {
    info "正在移除 Fish Shell 及其生态工具..."
    local all_users=($(get_all_real_users))
    
    # 1. 恢复所有真实用户的默认 Shell
    for user in "${all_users[@]}"; do
        if [[ "$user" != "root" ]]; then
            chsh -s /bin/bash "$user" 2>/dev/null || true
        fi
    done
    
    # 2. 物理清除包与二进制
    apt-get purge -yq fish zoxide
    rm -f /usr/local/bin/starship
    
    # 3. 彻底清除每个用户的 Fish 和 Starship 目录/链接
    for user in "${all_users[@]}"; do
        local user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/fish"
        rm -f "$user_home/.config/starship.toml"
    done
    success "Fish 及其配置已彻底清理。"
}

# ----------------- Micro 编辑器安装 -----------------
install_micro() {
    info "正在安装 Micro 编辑器 最新二进制版..."
    safe_apt_install xclip
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
    
    # 1. 为真理源用户物理生成配置目录与设置
    mkdir -p "$sot_home/.config/micro"
    render_template "templates/apps/devops/micro_settings.json" "$sot_home/.config/micro/settings.json"
    chown -R "$sot_user:$sot_user" "$sot_home/.config/micro" 2>/dev/null || true

    # 2. 为真理源用户安装 filemanager 插件
    local run_cmd=()
    if [[ "$sot_user" != "root" ]]; then
        run_cmd=("sudo" "-H" "-u" "$sot_user")
    fi
    local micro_bin="/usr/local/bin/micro"
    [[ ! -f "$micro_bin" ]] && micro_bin=$(which micro 2>/dev/null || echo "micro")
    "${run_cmd[@]}" "$micro_bin" -plugin install filemanager >/dev/null 2>&1 || true

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
    apt-get purge -yq micro xclip
    
    # 清理所有用户的配置文件
    local all_users=($(get_all_real_users))
    for user in "${all_users[@]}"; do
        local user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/micro"
    done
    
    rm -f /etc/profile.d/micro_env.sh
    remove_fish_env "MICRO_TRUECOLOR"
    remove_fish_env "EDITOR"
    remove_fish_env "VISUAL"
    success "Micro 已彻底移除。"
}

# ----------------- Acme.sh 证书工具安装 -----------------
install_acme() {
    info "正在部署 Acme.sh 自动化证书管理工具..."
    safe_apt_install socat openssl cron
    local email
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：跳过邮箱输入。"
        email=""
    else
        read -p "请输入用于接收证书提醒的邮箱 (可选): " email
    fi

    local target_user
    target_user=$(get_initial_user)
    local target_home
    target_home=$(eval echo "~$target_user")
    
    local run_cmd=()
    if [[ "$target_user" != "root" ]]; then
        run_cmd=("sudo" "-H" "-u" "$target_user")
    fi

    local tmp_acme="/tmp/acme_install.sh"
    if download_with_fallback "$tmp_acme" "https://get.acme.sh"; then
        if [[ -z "$email" ]]; then
            "${run_cmd[@]}" bash "$tmp_acme" | "${run_cmd[@]}" sh -s email=admin@example.com
        else
            "${run_cmd[@]}" bash "$tmp_acme" | "${run_cmd[@]}" sh -s email="$email"
        fi
        rm -f "$tmp_acme"
    fi
    
    if [[ -d "$target_home/.acme.sh" ]]; then
        "${run_cmd[@]}" "$target_home/.acme.sh/acme.sh" --set-default-ca --server letsencrypt || true
    fi
    
    update_fish_path "$target_home/.acme.sh"
    success "Acme.sh 部署完成。"
}

uninstall_acme() {
    info "正在彻底移除 Acme.sh..."
    
    local normal_user
    normal_user=$(get_normal_user)
    local normal_home=""
    [[ -n "$normal_user" ]] && normal_home=$(eval echo "~$normal_user")

    # 1. 尝试安全调用所有潜在环境中的官方卸载程序
    local acme_paths=()
    [[ -f "$HOME/.acme.sh/acme.sh" ]] && acme_paths+=("$HOME/.acme.sh/acme.sh")
    [[ -f "/root/.acme.sh/acme.sh" ]] && acme_paths+=("/root/.acme.sh/acme.sh")
    [[ -n "$normal_home" && -f "$normal_home/.acme.sh/acme.sh" ]] && acme_paths+=("$normal_home/.acme.sh/acme.sh")

    for acme_bin in "${acme_paths[@]}"; do
        info "调用官方卸载程序: $acme_bin ..."
        "$acme_bin" --uninstall >/dev/null 2>&1 || true
    done

    # 2. 物理清除所有可能的安装路径
    info "正在物理清除相关目录与二进制文件..."
    rm -rf "$HOME/.acme.sh"
    rm -rf "/root/.acme.sh"
    [[ -n "$normal_home" ]] && rm -rf "$normal_home/.acme.sh"
    
    # 清除可能的系统全局软链接
    rm -f /usr/local/bin/acme.sh
    rm -f /usr/bin/acme.sh

    # 3. 彻底清理用户的 Shell 环境变量与别名 (Bash / Fish / Zsh)
    remove_fish_path "\$HOME/.acme.sh"
    [[ -n "$normal_home" ]] && remove_fish_path "\$normal_home/.acme.sh"

    local profiles=()
    profiles+=("$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "/root/.bashrc" "/root/.bash_profile" "/root/.zshrc")
    [[ -n "$normal_home" ]] && profiles+=("$normal_home/.bashrc" "$normal_home/.bash_profile" "$normal_home/.zshrc")

    for profile in "${profiles[@]}"; do
        if [[ -f "$profile" ]]; then
            # 自动擦除所有包含 acme.sh 的别名与环境导入配置
            sed -i '/acme\.sh/d' "$profile" 2>/dev/null || true
        fi
    done

    # 4. 彻底清理所有用户的 Cron 定时任务残留
    info "正在清理定时任务残留..."
    if crontab -l >/dev/null 2>&1; then
        crontab -l | grep -v "acme.sh" | crontab - || true
    fi
    if [[ -n "$normal_user" ]]; then
        if crontab -u "$normal_user" -l >/dev/null 2>&1; then
            crontab -u "$normal_user" -l | grep -v "acme.sh" | crontab -u "$normal_user" - || true
        fi
    fi

    success "Acme.sh 已彻底从系统中清空。"
}
