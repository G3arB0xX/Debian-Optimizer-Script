#!/bin/bash
# =========================================================
# 运维与终端增强模块 (Fish, Micro, Acme.sh)
# =========================================================

# ----------------- Fish Shell 安装与增强 -----------------
install_fish() {
    info "正在安装 Fish Shell 及其生态工具..."
    
    # 1. 安装本体与核心依赖
    apt-get update -yq
    apt-get install -yq fish fzf fd-find curl git
    
    # 2. 安装 zoxide 与 Starship (现代化的二进制工具)
    if ! command -v zoxide >/dev/null 2>&1; then
        info "正在安装 zoxide..."
        if ! apt-get install -yq zoxide 2>/dev/null; then
            curl -sS https://zoxide.xyz/install.sh | bash -s -- -y >/dev/null 2>&1
            cp "$HOME/.local/bin/zoxide" /usr/local/bin/ 2>/dev/null || true
        fi
    fi

    if ! command -v starship >/dev/null 2>&1; then
        info "正在安装 Starship Prompt..."
        curl -sS https://starship.rs/install.sh | sh -s -- -y >/dev/null 2>&1
    fi

    local normal_user
    normal_user=$(get_normal_user)
    local users=()
    users+=("root")
    [[ -n "$normal_user" ]] && users+=("$normal_user")

    for user in "${users[@]}"; do
        info "正在为用户 $user 配置 Fish 环境..."
        local user_home
        user_home=$(eval echo "~$user")
        local fish_conf_dir="$user_home/.config/fish"
        local functions_dir="$fish_conf_dir/functions"
        local conf_d_dir="$fish_conf_dir/conf.d"

        mkdir -p "$functions_dir" "$conf_d_dir"

        # 安装 Fisher 插件管理器
        if [[ ! -f "$functions_dir/fisher.fish" ]]; then
            info "正在为 $user 部署 Fisher..."
            sudo -u "$user" fish -c "curl -sL https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish | source && fisher install jorgebucaran/fisher"
        fi
        
        # 部署核心插件 (fzf.fish, autopair, zoxide, puffer-fish, replay)
        info "正在为 $user 部署高级插件集..."
        sudo -u "$user" fish -c "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish"
        
        # 智能配置文件加载 (利用 conf.d 机制)
        
        # 1. zoxide 初始化
        echo 'if command -v zoxide >/dev/null 2>&1; zoxide init fish | source; end' > "$conf_d_dir/zoxide.fish"
        
        # 2. Starship 初始化与主题预设 (Gruvbox Rainbow)
        echo 'if command -v starship >/dev/null 2>&1; starship init fish | source; end' > "$conf_d_dir/starship.fish"
        mkdir -p "$user_home/.config"
        sudo -u "$user" starship preset gruvbox-rainbow -o "$user_home/.config/starship.toml"
        
        # 3. 移除问候语禁用配置 (恢复 Fish 默认问候语)
        rm -f "$conf_d_dir/greeting.fish"
        
        # 4. 基础 Abbreviation (架构师常用)
        cat > "$conf_d_dir/abbrs.fish" << 'EOF'
if status is-interactive
    abbr -a l 'ls -lah'
    abbr -a .. 'cd ..'
    abbr -a ... 'cd ../..'
    abbr -a gs 'git status'
    abbr -a gd 'git diff'
    abbr -a gaa 'git add .'
    abbr -a gc 'git commit -m'
    abbr -a gp 'git push'
end
EOF
        
        # 修复权限
        chown -R "$user:$user" "$fish_conf_dir"

        # 验证配置文件正确性
        if ! sudo -u "$user" fish -n "$fish_conf_dir/config.fish" 2>/dev/null; then
            err "用户 $user 的 Fish 配置文件语法校验失败！"
        else
            info "✅ 用户 $user 的 Fish 配置校验通过。"
        fi
    done

    # 3. 设置默认 Shell (仅针对普通用户，root 保留 bash)
    if [[ -n "$normal_user" ]]; then
        info "正在将普通用户 $normal_user 的默认 Shell 设置为 Fish..."
        chsh -s "$(which fish)" "$normal_user"
        info "✅ $normal_user 默认 Shell 已切换。Root 保持为 Bash 以确保紧急维护安全。"
    else
        warn "未检测到普通用户，跳过默认 Shell 切换。Root 建议保持使用 Bash。"
    fi
    
    info "✅ Fish Shell 增强版环境部署完成。"
}

uninstall_fish() {
    info "正在移除 Fish Shell 及其生态工具..."
    
    local normal_user
    normal_user=$(get_normal_user)
    if [[ -n "$normal_user" ]] ; then
        chsh -s /bin/bash "$normal_user"
    fi
    
    apt-get purge -yq fish zoxide
    rm -f /usr/local/bin/starship
    
    # 清理所有用户的配置
    local users=("root")
    [[ -n "$normal_user" ]] && users+=("$normal_user")
    for user in "${users[@]}"; do
        local user_home=$(eval echo "~$user")
        rm -rf "$user_home/.config/fish"
        rm -f "$user_home/.config/starship.toml"
    done
    
    info "✅ Fish 及其配置已彻底清理，普通用户已回退到 Bash。"
}

# ----------------- Micro 编辑器安装 -----------------
install_micro() {
    info "正在安装 Micro 编辑器 (最新二进制版)..."
    
    # 安装剪贴板支持依赖
    apt-get install -yq xclip
    
    # 使用官方安装脚本拉取最新二进制
    curl https://getmic.ro | bash
    mv micro /usr/local/bin/
    
    # 安装插件 (filemanager)
    info "正在安装 Micro 插件: filemanager..."
    micro -plugin install filemanager
    
    # 配置基础生态与进阶增强设置 (社区最佳实践)
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

    # 注入全局环境变量 (真彩色支持与默认编辑器)
    info "配置系统环境变量 (True Color & Default Editor)..."
    local profile_file="/etc/profile.d/micro_env.sh"
    cat > "$profile_file" << 'EOF'
export MICRO_TRUECOLOR=1
export EDITOR=micro
export VISUAL=micro
EOF
    chmod +x "$profile_file"
    
    # 同步至 Fish
    update_fish_env "MICRO_TRUECOLOR" "1"
    update_fish_env "EDITOR" "micro"
    update_fish_env "VISUAL" "micro"
    
    # 尝试使用 update-alternatives 注册为系统编辑器
    update-alternatives --install /usr/bin/editor editor /usr/local/bin/micro 100 || true
    update-alternatives --set editor /usr/local/bin/micro || true
    
    info "✅ Micro 进阶优化配置完成。已开启社区最佳实践设置。"
}

uninstall_micro() {
    info "正在移除 Micro..."
    rm -f /usr/local/bin/micro
    rm -rf "$HOME/.config/micro"
    rm -f /etc/profile.d/micro_env.sh
    update-alternatives --remove editor /usr/local/bin/micro || true
    apt-get purge -yq xclip
    
    # 清理 Fish 环境
    remove_fish_env "MICRO_TRUECOLOR"
    remove_fish_env "EDITOR"
    remove_fish_env "VISUAL"

    info "✅ Micro 已彻底移除，环境已清理。"
}

# ----------------- Acme.sh 证书工具安装 -----------------
install_acme() {
    info "正在部署 Acme.sh 自动化证书管理工具..."
    
    # 依赖检查
    apt-get install -yq socat openssl cron
    
    # 执行安装
    read -p "请输入用于接收证书提醒的邮箱 (可选): " email
    if [[ -z "$email" ]]; then
        curl https://get.acme.sh | sh -s email=admin@example.com
    else
        curl https://get.acme.sh | sh -s email="$email"
    fi
    
    # 环境变量注入
    source "$HOME/.acme.sh/acme.sh.env" || true
    
    # 切换默认 CA 为 Let's Encrypt (提高兼容性)
    "$HOME/.acme.sh/acme.sh" --set-default-ca --server letsencrypt
    
    # 同步至 Fish 环境
    update_fish_path "\$HOME/.acme.sh"

    info "✅ Acme.sh 部署完成，默认 CA 已切换为 Let's Encrypt。"
}

uninstall_acme() {
    info "正在移除 Acme.sh..."
    [[ -f "$HOME/.acme.sh/acme.sh" ]] && "$HOME/.acme.sh/acme.sh" --uninstall
    rm -rf "$HOME/.acme.sh"
    
    # 清理 Fish 环境
    remove_fish_path "\$HOME/.acme.sh"

    info "✅ Acme.sh 已卸载。"
}
