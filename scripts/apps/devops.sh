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

    local normal_user
    normal_user=$(get_normal_user)
    local users=("root")
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    export PATH=$PATH:/usr/local/bin

    for user in "${users[@]}"; do
        info "正在为用户 $user 配置 Fish 环境..."
        local user_home
        user_home=$(eval echo "~$user")
        [[ ! -d "$user_home" ]] && continue

        # 智能环境适配：若当前目标用户为 root，则直接执行以避开容器内可能缺失 sudo 的崩溃问题；否则使用 sudo 降权执行
        local run_cmd=()
        if [[ "$user" != "root" ]]; then
            run_cmd=("sudo" "-H" "-u" "$user")
        fi

        local fish_conf_dir="$user_home/.config/fish"
        local functions_dir="$fish_conf_dir/functions"
        local conf_d_dir="$fish_conf_dir/conf.d"

        mkdir -p "$functions_dir" "$conf_d_dir"
        chown -R "$user:$user" "$user_home/.config" 2>/dev/null || true

        # 1. 安装 Fisher 插件管理器 (改为非交互静默模式)
        if [[ ! -f "$functions_dir/fisher.fish" ]]; then
            info "正在为 $user 部署 Fisher..."
            local tmp_fisher="/tmp/fisher.fish"
            if download_with_fallback "$tmp_fisher" "https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish"; then
                "${run_cmd[@]}" fish -c "source $tmp_fisher && fisher install jorgebucaran/fisher" >/dev/null 2>&1 || true
                rm -f "$tmp_fisher"
            fi
        fi
        
        # 2. 部署核心插件集
        info "正在为 $user 部署高级插件集..."
        "${run_cmd[@]}" fish -c "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish" >/dev/null 2>&1 || true
        
        # 3. 配置文件加载
        cat > "$conf_d_dir/zoxide.fish" << 'EOF'
if command -v zoxide >/dev/null 2>&1
    zoxide init fish | source
else if test -f /usr/local/bin/zoxide
    /usr/local/bin/zoxide init fish | source
end
EOF
        cat > "$conf_d_dir/starship.fish" << 'EOF'
if command -v starship >/dev/null 2>&1
    starship init fish | source
else if test -f /usr/local/bin/starship
    /usr/local/bin/starship init fish | source
end
EOF
        
        # 应用主题预设 (确保 starship 可达)
        local starship_bin="/usr/local/bin/starship"
        [[ ! -f "$starship_bin" ]] && starship_bin=$(which starship 2>/dev/null || echo "starship")
        
        if command -v "$starship_bin" >/dev/null 2>&1; then
            info "应用 Starship Gruvbox-Rainbow 主题..."
            "${run_cmd[@]}" "$starship_bin" preset gruvbox-rainbow -o "$user_home/.config/starship.toml" >/dev/null 2>&1 || true
        fi
        
        # 基础 Abbreviation
        cat > "$conf_d_dir/abbrs.fish" << EOF
if status is-interactive
    abbr -a l 'ls -lah'
    abbr -a .. 'cd ..'
    abbr -a ... 'cd ../..'
    abbr -a gs 'git status'
    abbr -a gd 'git diff'
    abbr -a gaa 'git add .'
    abbr -a gc 'git commit -m'
    abbr -a gp 'git push'
    abbr -a debopti '/usr/local/bin/debopti'
end
EOF
        # 终极权限修复
        chown -R "$user:$user" "$user_home/.config" 2>/dev/null || true
    done

    # 3. 设置默认 Shell
    if [[ -n "$normal_user" ]]; then
        info "正在将普通用户 $normal_user 的默认 Shell 设置为 Fish..."
        local fish_path=$(which fish 2>/dev/null || echo "/usr/bin/fish")
        chsh -s "$fish_path" "$normal_user"
    fi
    
    success "Fish Shell 现代化环境部署完成。"
}

uninstall_fish() {
    info "正在移除 Fish Shell 及其生态工具..."
    local normal_user=$(get_normal_user)
    [[ -n "$normal_user" ]] && chsh -s /bin/bash "$normal_user"
    apt-get purge -yq fish zoxide
    rm -f /usr/local/bin/starship
    local users=("root")
    [[ -n "$normal_user" ]] && users+=("$normal_user")
    for user in "${users[@]}"; do
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
                    # info "Micro 官方脚本安装成功。"
                else
                    die "Micro 脚本执行或移动文件失败！"
                fi
            else
                die "Micro 官方脚本下载失败，无法完成安装！"
            fi
        # else
            # info "Micro 通过 APT 安装成功。"
        fi
    else
        info "Micro 已安装，跳过此步骤。"
    fi
    micro -plugin install filemanager || true
    mkdir -p "$HOME/.config/micro"
    cat > "$HOME/.config/micro/settings.json" << 'EOF'
{
    "colorscheme": "simple",
    "mouse": true,
    "savecursor": true,
    "saveundo": true,
    "scrollbar": true,
    "tabsize": 4,
    "autoindent": true,
    "autosu": true,
    "cursorline": true,
    "eofnewline": true,
    "fastdirty": true,
    "mkparents": true,
    "rmtrailingws": true,
    "softwrap": true,
    "tabstospaces": true,
    "wordwrap": true,
    "basename": true,
    "ignorecase": true,
    "matchbrace": true,
    "matchbracewait": "50ms",
    "ruler": true,
    "incsearch": true,
    "smartpaste": true,
    "sucmd": "sudo"
}
EOF
    local profile_file="/etc/profile.d/micro_env.sh"
    cat > "$profile_file" << 'EOF'
export MICRO_TRUECOLOR=1
export EDITOR=micro
export VISUAL=micro
EOF
    chmod +x "$profile_file"
    update_fish_env "MICRO_TRUECOLOR" "1"
    update_fish_env "EDITOR" "micro"
    update_fish_env "VISUAL" "micro"
    update-alternatives --install /usr/bin/editor editor /usr/local/bin/micro 100 || true
    update-alternatives --set editor /usr/local/bin/micro || true
    success "Micro 进阶优化配置完成。"
}

uninstall_micro() {
    info "正在移除 Micro..."
    rm -f /usr/local/bin/micro
    rm -rf "$HOME/.config/micro"
    rm -f /etc/profile.d/micro_env.sh
    update-alternatives --remove editor /usr/local/bin/micro || true
    apt-get purge -yq xclip
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

    local tmp_acme="/tmp/acme_install.sh"
    if download_with_fallback "$tmp_acme" "https://get.acme.sh"; then
        if [[ -z "$email" ]]; then
            bash "$tmp_acme" | sh -s email=admin@example.com
        else
            bash "$tmp_acme" | sh -s email="$email"
        fi
        rm -f "$tmp_acme"
    fi
    source "$HOME/.acme.sh/acme.sh.env" || true
    "$HOME/.acme.sh/acme.sh" --set-default-ca --server letsencrypt || true
    update_fish_path "\$HOME/.acme.sh"
    success "Acme.sh 部署完成。"
}

uninstall_acme() {
    info "正在移除 Acme.sh..."
    [[ -f "$HOME/.acme.sh/acme.sh" ]] && "$HOME/.acme.sh/acme.sh" --uninstall
    rm -rf "$HOME/.acme.sh"
    remove_fish_path "\$HOME/.acme.sh"
    success "Acme.sh 已卸载。"
}
