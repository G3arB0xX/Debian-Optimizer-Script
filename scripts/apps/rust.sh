#!/bin/bash
# =========================================================
# Rust 运行环境与中转生态模块 Realm
# =========================================================

# ----------------- Rust 环境部署 -----------------
install_rust() {
    info "正在安装/修复 Rust 编译与运行环境 rustup..."
    
    # 确保加载最新配置以支持状态读取
    [[ -f "${CONFIG_FILE:-/etc/debopti/debopti.conf}" ]] && source "${CONFIG_FILE:-/etc/debopti/debopti.conf}"

    # 0. 补齐系统 C/C++ 编译与链接工具链，解决无 cc 链接器警告，并记录状态用于原子化卸载
    if ! dpkg -s "build-essential" >/dev/null 2>&1; then
        info "检测到系统缺失 C/C++ 编译工具链，正在自动补齐 build-essential..."
        safe_apt_install "build-essential" || { err "编译工具链 build-essential 安装失败，可能会影响 Rust 依赖编译。"; return 1; }
        save_project_config "RUST_INSTALLED_BUILD_ESSENTIAL" "true"
    fi

    # 1. 确定目标用户及家目录，解决 sudo 执行时 $HOME 冲突与权限污染
    local target_user
    target_user=$(get_initial_user)
    local user_home
    user_home=$(eval echo "~$target_user")

    # 2. 国内镜像加速配置
    local rustup_dist=""
    local rustup_update=""
    if [[ "$IS_CN_REGION" == "true" ]]; then
        info "检测到大陆环境，自动注入字节跳动 (rsproxy) 镜像加速..."
        rustup_dist="https://rsproxy.cn"
        rustup_update="https://rsproxy.cn/rustup"
    fi

    # 3. 获取 rustup-init
    local tmp_init="/tmp/rustup-init"
    local download_url="https://static.rust-lang.org/rustup/dist/$(uname -m)-unknown-linux-gnu/rustup-init"
    [[ "$IS_CN_REGION" == "true" ]] && download_url="https://rsproxy.cn/rustup/dist/$(uname -m)-unknown-linux-gnu/rustup-init"
    
    download_with_fallback "$tmp_init" "$download_url" || return 1
    chmod +x "$tmp_init"

    # 4. 执行 non-interactive 降权安全安装
    info "执行 Rustup 核心安装 (用户: $target_user, 家目录: $user_home)..."
    if [[ "$target_user" != "root" ]]; then
        # 作为普通用户执行安装，并传递加速变量
        sudo -u "$target_user" env HOME="$user_home" \
            RUSTUP_DIST_SERVER="$rustup_dist" \
            RUSTUP_UPDATE_ROOT="$rustup_update" \
            "$tmp_init" -y --default-toolchain stable --profile minimal --no-modify-path || { err "Rustup 安装失败。"; rm -f "$tmp_init"; return 1; }
    else
        # 作为 root 执行安装，显式指定 HOME 变量防止 mismatch 报错
        env HOME="/root" \
            RUSTUP_DIST_SERVER="$rustup_dist" \
            RUSTUP_UPDATE_ROOT="$rustup_update" \
            "$tmp_init" -y --default-toolchain stable --profile minimal --no-modify-path || { err "Rustup 安装失败。"; rm -f "$tmp_init"; return 1; }
    fi
    
    # 5. 环境注入与 Cargo 镜像配置
    mkdir -p "$user_home/.cargo"
    if [[ "$IS_CN_REGION" == "true" ]]; then
        cat > "$user_home/.cargo/config.toml" << EOF
[source.crates-io]
replace-with = 'rsproxy'
[source.rsproxy]
registry = "https://rsproxy.cn/crates.io-index"
[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
[registries.rsproxy]
index = "https://rsproxy.cn/crates.io-index"
[net]
git-fetch-with-cli = true
EOF
    fi

    # 修正可能存在的权限污染，确保普通用户对其配置具有完整控制权
    if [[ "$target_user" != "root" ]]; then
        chown -R "$target_user:$target_user" "$user_home/.cargo" "$user_home/.rustup" 2>/dev/null || true
    fi

    rm -f "$tmp_init"
    
    # 6. 同步至 Fish 环境
    update_fish_path "\$HOME/.cargo/bin"
    
    # 动态获取当前实际安装的 rustc 版本
    local rustc_ver="未知"
    if [[ "$target_user" != "root" ]]; then
        rustc_ver=$(sudo -u "$target_user" env HOME="$user_home" "$user_home/.cargo/bin/rustc" --version 2>/dev/null | awk '{print $2}' || echo "未知")
    else
        rustc_ver=$(env HOME="/root" "/root/.cargo/bin/rustc" --version 2>/dev/null | awk '{print $2}' || echo "未知")
    fi

    success "Rust 环境已就绪。版本: $rustc_ver"
}

uninstall_rust() {
    info "正在移除 Rust 环境及其所有组件..."

    # 确保加载最新配置以支持状态读取
    [[ -f "${CONFIG_FILE:-/etc/debopti/debopti.conf}" ]] && source "${CONFIG_FILE:-/etc/debopti/debopti.conf}"

    local target_user
    target_user=$(get_initial_user)
    local user_home
    user_home=$(eval echo "~$target_user")

    # 1. 移除普通用户的 Rust 环境
    if [[ "$target_user" != "root" ]]; then
        if sudo -u "$target_user" env HOME="$user_home" command -v rustup >/dev/null 2>&1; then
            sudo -u "$target_user" env HOME="$user_home" rustup self uninstall -y >/dev/null 2>&1
        fi
        rm -rf "$user_home/.cargo" "$user_home/.rustup"
    fi

    # 2. 移除 Root 用户的 Rust 环境 (仅在具有 root 权限时执行，防止沙盒测试越权报错)
    if [[ $EUID -eq 0 ]]; then
        if env HOME="/root" command -v rustup >/dev/null 2>&1; then
            env HOME="/root" rustup self uninstall -y >/dev/null 2>&1
        fi
        rm -rf "/root/.cargo" "/root/.rustup"
    fi

    # 3. 如果是由本脚本安装的 build-essential，则一并卸载以保持原子化还原
    if [[ "${RUST_INSTALLED_BUILD_ESSENTIAL:-}" == "true" ]]; then
        info "检测到 build-essential 是由本模块安装的，正在执行原子化卸载..."
        apt-get purge -yq build-essential >/dev/null 2>&1 || true
        apt-get autoremove -yq >/dev/null 2>&1 || true
        save_project_config "RUST_INSTALLED_BUILD_ESSENTIAL" "false"
    fi
    
    # 清理 Fish 环境
    remove_fish_path "\$HOME/.cargo/bin"
    
    success "Rust 已从系统彻底移除。"
}
