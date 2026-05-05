#!/bin/bash
# =========================================================
# Rust 运行环境与中转生态模块 (Realm)
# =========================================================

# ----------------- Rust 环境部署 -----------------
install_rust() {
    info "正在安装/修复 Rust 编译与运行环境 (rustup)..."
    
    # 1. 国内镜像加速配置
    if [[ "$IS_CN_REGION" == "true" ]]; then
        info "检测到大陆环境，自动注入字节跳动 (rsproxy) 镜像加速..."
        export RUSTUP_DIST_SERVER="https://rsproxy.cn"
        export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
    fi

    # 2. 获取 rustup-init
    local tmp_init="/tmp/rustup-init"
    local download_url="https://static.rust-lang.org/rustup/dist/$(uname -m)-unknown-linux-gnu/rustup-init"
    
    download_with_fallback "$tmp_init" "$download_url" || return 1
    chmod +x "$tmp_init"

    # 3. 非交互式安装 (默认 profile: minimal 以节省空间和时间)
    "$tmp_init" -y --default-toolchain stable --profile minimal --no-modify-path
    
    # 4. 环境注入
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env" || true
    
    # 国内环境注入 cargo 镜像配置
    if [[ "$IS_CN_REGION" == "true" ]]; then
        mkdir -p "$HOME/.cargo"
        cat > "$HOME/.cargo/config.toml" << EOF
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

    rm -f "$tmp_init"
    info "✅ Rust 环境已就绪。版本: $(rustc --version 2>/dev/null || echo '未知')"
}

uninstall_rust() {
    info "正在移除 Rust 环境及其所有组件..."
    if command -v rustup >/dev/null 2>&1; then
        rustup self uninstall -y
    fi
    rm -rf "$HOME/.cargo" "$HOME/.rustup"
    info "✅ Rust 已从系统彻底移除。"
}
