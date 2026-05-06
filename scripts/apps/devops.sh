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
        curl -sS https://zoxide.xyz/install.sh | bash -s -- -y >/dev/null 2>&1 || true
        command -v zoxide >/dev/null 2>&1 || safe_apt_install zoxide
    fi

    if [[ ! -f "/usr/local/bin/starship" ]]; then
        info "正在安装 Starship Prompt..."
        curl -sS https://starship.rs/install.sh | sh -s -- -y >/dev/null 2>&1
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
        local fish_conf_dir="$user_home/.config/fish"
        local functions_dir="$fish_conf_dir/functions"
        local conf_d_dir="$fish_conf_dir/conf.d"

        mkdir -p "$functions_dir" "$conf_d_dir"
        mkdir -p "$user_home/.config"
        # 提前修复权限，确保用户有权写入配置
        chown -R "$user:$user" "$user_home/.config"

        # 1. 安装 Fisher 插件管理器
        if [[ ! -f "$functions_dir/fisher.fish" ]]; then
            info "正在为 $user 部署 Fisher..."
            sudo -u "$user" fish -c "curl -sL https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish | source && fisher install jorgebucaran/fisher" || true
        fi
        
        # 2. 部署核心插件集
        info "正在为 $user 部署高级插件集..."
        sudo -u "$user" fish -c "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish" || true
        
        # 3. 配置文件加载
        echo 'if command -v zoxide >/dev/null 2>&1; zoxide init fish | source; else if test -f /usr/local/bin/zoxide; /usr/local/bin/zoxide init fish | source; end' > "$conf_d_dir/zoxide.fish"
        echo 'if command -v starship >/dev/null 2>&1; starship init fish | source; else if test -f /usr/local/bin/starship; /usr/local/bin/starship init fish | source; end' > "$conf_d_dir/starship.fish"
        
        # 应用主题预设 (使用绝对路径)
        if [[ -f "/usr/local/bin/starship" ]]; then
            sudo -u "$user" /usr/local/bin/starship preset gruvbox-rainbow -o "$user_home/.config/starship.toml" || true
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
        # 再次修复权限 (针对新生成的文件)
        chown -R "$user:$user" "$user_home/.config"
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
    curl https://getmic.ro | bash
    mv micro /usr/local/bin/
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
    read -p "请输入用于接收证书提醒的邮箱 (可选): " email
    if [[ -z "$email" ]]; then
        curl https://get.acme.sh | sh -s email=admin@example.com
    else
        curl https://get.acme.sh | sh -s email="$email"
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
