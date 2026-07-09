#!/bin/bash
# =========================================================
# 运维与终端增强模块 Fish, Micro, Lego
# =========================================================

SOT_KNOWN_USERS_FILE="/etc/debopti/sot_known_users"

# Podman 代管 Fish 片段：仅 SOT 用户与 root 同步，不暴露给其他用户
PODMAN_MAINTAINER_FISH_FILES=(
    debopti_apppod.fish
    debopti_appctl.fish
    debopti_applog.fish
    debopti_appshell.fish
)

_is_podman_maintainer_user() {
    local u=$1
    local sot_user=$2
    [[ "$u" == "root" || "$u" == "$sot_user" ]]
}

_fish_conf_d_is_podman_maintainer_only() {
    local base=$1
    local f
    for f in "${PODMAN_MAINTAINER_FISH_FILES[@]}"; do
        [[ "$base" == "$f" ]] && return 0
    done
    return 1
}

# 允许其他用户遍历 SOT 家目录以跟随软链接（只读共享）
apply_sot_home_traverse_permissions() {
    local sot_home=$1
    [[ -d "$sot_home" ]] || return 0
    chmod o+rx "$sot_home" 2>/dev/null || true
    [[ -d "$sot_home/.config" ]] && chmod o+rx "$sot_home/.config" 2>/dev/null || true
}

# SOT 配置目录：属主可写，其他用户只读（o+r / o+rX，清除 o+w）
apply_sot_shared_readonly() {
    local path=$1
    local owner=$2
    [[ -d "$path" ]] || return 0
    chown -R "$owner:$owner" "$path" 2>/dev/null || true
    find "$path" -type d -exec chmod o=rX {} + 2>/dev/null || true
    find "$path" -type f -exec chmod o=r {} + 2>/dev/null || true
    chmod -R o-w "$path" 2>/dev/null || true
}

# 将 SOT 用户 Fish 从旧版 /etc/fish/shared_sot 软链迁移为物理目录
_migrate_sot_fish_from_shared_sot() {
    local sot_user=$1
    local sot_home=$2
    local sot_fish="$sot_home/.config/fish"

    if [[ -L "$sot_fish" ]] && [[ "$(readlink "$sot_fish" 2>/dev/null || true)" == *"shared_sot"* ]]; then
        rm -f "$sot_fish"
    fi
    if [[ ! -d "$sot_fish" && -d /etc/fish/shared_sot ]]; then
        mkdir -p "$sot_home/.config"
        cp -a /etc/fish/shared_sot "$sot_fish"
        chown -R "$sot_user:$sot_user" "$sot_fish"
    fi
}

# 非 SOT 用户：死配置经 SOT 只读同步；历史/通用变量/插件列表等可变内容保留本地
_link_user_fish_hybrid() {
    local u=$1
    local u_home=$2
    local sot_home=$3
    local sot_user=$4
    local sot_fish="$sot_home/.config/fish"
    local user_fish="$u_home/.config/fish"
    local user_conf_d="$user_fish/conf.d"
    local sot_conf_d="$sot_fish/conf.d"

    [[ -d "$sot_fish" ]] || return 0

    if [[ -L "$user_fish" ]]; then
        rm -f "$user_fish"
    fi
    mkdir -p "$user_fish/conf.d" "$user_fish/functions" "$user_fish/conf.d.local"
    chown -R "$u:$u" "$user_fish" 2>/dev/null || true

    # 拆除旧版整目录软链，改为本地可写目录
    local sub
    for sub in conf.d functions completions themes; do
        if [[ -L "$user_fish/$sub" ]]; then
            rm -f "$user_fish/$sub"
            mkdir -p "$user_fish/$sub"
            chown "$u:$u" "$user_fish/$sub" 2>/dev/null || true
        fi
    done

    # conf.d：仅为 SOT 托管文件建软链；用户自建的同名实体文件不覆盖
    if [[ -d "$sot_conf_d" ]]; then
        local f base target
        for f in "$sot_conf_d"/*.fish; do
            [[ -f "$f" ]] || continue
            base=$(basename "$f")
            target="$user_conf_d/$base"
            if _fish_conf_d_is_podman_maintainer_only "$base"; then
                if ! _is_podman_maintainer_user "$u" "$sot_user"; then
                    rm -f "$target"
                    continue
                fi
            fi
            if [[ -e "$target" && ! -L "$target" ]]; then
                continue
            fi
            ln -sf "$f" "$target"
            chown -h "$u:$u" "$target" 2>/dev/null || true
        done
    fi

    # SOT autoload 路径引导（本地 conf.d/00-debopti-sot-bootstrap.fish）
    render_template "templates/apps/devops/fish_sot_bootstrap.fish" \
        "$user_conf_d/00-debopti-sot-bootstrap.fish" "SOT_FISH=$sot_fish"
    chown "$u:$u" "$user_conf_d/00-debopti-sot-bootstrap.fish" 2>/dev/null || true

    # fish_plugins：本地副本（fisher 会改写；不可只读软链）
    if [[ -f "$sot_fish/fish_plugins" && ! -f "$user_fish/fish_plugins" ]]; then
        cp -a "$sot_fish/fish_plugins" "$user_fish/fish_plugins"
        chown "$u:$u" "$user_fish/fish_plugins" 2>/dev/null || true
    fi

    # 通用变量（-U / abbr 持久化等）— 每用户独立
    if [[ ! -f "$user_fish/fish_variables" ]]; then
        touch "$user_fish/fish_variables"
    fi
    chown "$u:$u" "$user_fish/fish_variables" 2>/dev/null || true
    chmod 600 "$user_fish/fish_variables" 2>/dev/null || true

    # 命令历史默认在 ~/.local/share/fish/fish_history（XDG_DATA_HOME），不在 ~/.config/fish
    mkdir -p "$u_home/.local/share/fish"
    chown -R "$u:$u" "$u_home/.local/share/fish" 2>/dev/null || true
}

# 清理无人引用的旧版 Fish 中间层目录
_cleanup_legacy_fish_shared_sot() {
    [[ -d /etc/fish/shared_sot ]] || return 0
    local u u_home target
    for u in $(get_all_real_users); do
        u_home=$(eval echo "~$u")
        [[ -L "$u_home/.config/fish" ]] || continue
        target=$(readlink "$u_home/.config/fish" 2>/dev/null || true)
        if [[ "$target" == "/etc/fish/shared_sot" || "$target" == *"shared_sot" ]]; then
            return 0
        fi
    done
    rm -rf /etc/fish/shared_sot
}

# 为所有真实用户同步 Fish / Micro / Yazi 的 SOT 只读软链接
# 参数: $1=true 时减少日志输出（启动钩子用）
sync_devops_sot_links() {
    local quiet=${1:-false}
    local sot_user sot_home
    sot_user=$(get_sot_user)
    sot_home=$(eval echo "~$sot_user")
    [[ -d "$sot_home" ]] || return 0

    local has_fish=false has_micro=false has_yazi=false
    if [[ -d "$sot_home/.config/fish" ]] && command -v fish >/dev/null 2>&1; then
        has_fish=true
    fi
    [[ -d "$sot_home/.config/micro" ]] && has_micro=true
    [[ -d "$sot_home/.config/yazi" ]] && has_yazi=true
    if [[ "$has_fish" == false && "$has_micro" == false && "$has_yazi" == false ]]; then
        return 0
    fi

    [[ "$quiet" != "true" ]] && info "正在同步 DevOps SOT 配置软链接（只读共享）..."

    _migrate_sot_fish_from_shared_sot "$sot_user" "$sot_home"
    apply_sot_home_traverse_permissions "$sot_home"
    if [[ "$has_fish" == true ]]; then
        apply_sot_shared_readonly "$sot_home/.config/fish" "$sot_user"
    fi
    if [[ "$has_micro" == true ]]; then
        apply_sot_shared_readonly "$sot_home/.config/micro" "$sot_user"
    fi
    if [[ "$has_yazi" == true ]]; then
        apply_sot_shared_readonly "$sot_home/.config/yazi" "$sot_user"
    fi

    local all_users=($(get_all_real_users))
    local u u_home
    for u in "${all_users[@]}"; do
        u_home=$(eval echo "~$u")
        [[ -d "$u_home" ]] || continue
        mkdir -p "$u_home/.config"
        chown "$u:$u" "$u_home/.config" 2>/dev/null || true

        if [[ "$has_fish" == true && -f /etc/starship.toml ]]; then
            rm -f "$u_home/.config/starship.toml"
            ln -sf /etc/starship.toml "$u_home/.config/starship.toml"
            chown -h "$u:$u" "$u_home/.config/starship.toml" 2>/dev/null || true
        fi

        if [[ "$u" == "$sot_user" ]]; then
            continue
        fi

        if [[ "$has_fish" == true ]]; then
            _link_user_fish_hybrid "$u" "$u_home" "$sot_home" "$sot_user"
        fi
        if [[ "$has_micro" == true ]]; then
            rm -rf "$u_home/.config/micro"
            ln -sf "$sot_home/.config/micro" "$u_home/.config/micro"
            chown -h "$u:$u" "$u_home/.config/micro" 2>/dev/null || true
        fi
        if [[ "$has_yazi" == true ]]; then
            rm -rf "$u_home/.config/yazi"
            ln -sf "$sot_home/.config/yazi" "$u_home/.config/yazi"
            chown -h "$u:$u" "$u_home/.config/yazi" 2>/dev/null || true
        fi
    done

    if [[ "$has_fish" == true ]]; then
        _cleanup_legacy_fish_shared_sot
    fi
}

# debopti 启动钩子：检测新用户并同步 SOT 软链接
maybe_sync_devops_sot_on_startup() {
    local all_users=($(get_all_real_users))
    local known_file="$SOT_KNOWN_USERS_FILE"
    local new_users=()
    local u

    mkdir -p /etc/debopti
    for u in "${all_users[@]}"; do
        if [[ ! -f "$known_file" ]] || ! grep -qxF "$u" "$known_file" 2>/dev/null; then
            new_users+=("$u")
        fi
    done

    if [[ ${#new_users[@]} -gt 0 ]]; then
        info "检测到新用户: ${new_users[*]}，正在同步 DevOps SOT 配置..."
    fi

    sync_devops_sot_links true
    printf '%s\n' "${all_users[@]}" > "$known_file"
}

# ----------------- DevOps 共享 CLI 依赖（Fish / Micro / Yazi 统一入口）-----------------
# 各模块重合依赖仅通过此处安装，避免重复逻辑与版本/路径冲突。

_apt_pkg_available() {
    apt-cache show "$1" >/dev/null 2>&1
}

_devops_version_ge() {
    local current=$1
    local minimum=$2
    [[ -n "$current" && -n "$minimum" ]] || return 1
    [[ "$(printf '%s\n%s\n' "$minimum" "$current" | sort -V | head -1)" == "$minimum" ]]
}

_devops_github_linux_arch() {
    case "$(uname -m)" in
        x86_64)  echo "amd64" ;;
        aarch64) echo "arm64" ;;
        armv7l|armhf) echo "armv7" ;;
        i386|i686) echo "386" ;;
        *) echo "" ;;
    esac
}

_devops_github_dpkg_arch() {
    case "$(uname -m)" in
        x86_64)  echo "amd64" ;;
        aarch64) echo "arm64" ;;
        armv7l|armhf) echo "armhf" ;;
        i386|i686) echo "i386" ;;
        *) echo "" ;;
    esac
}

_ensure_devops_fzf() {
    local min_version="0.53"
    if command -v fzf >/dev/null 2>&1; then
        local current
        current=$(fzf --version 2>/dev/null | awk '{print $1}')
        if _devops_version_ge "$current" "$min_version"; then
            return 0
        fi
        info "fzf 版本 ${current:-未知} 低于 ${min_version}，将从官方 Release 升级..."
    fi

    if _apt_pkg_available fzf; then
        safe_apt_install fzf || true
        if command -v fzf >/dev/null 2>&1; then
            local apt_ver
            apt_ver=$(fzf --version 2>/dev/null | awk '{print $1}')
            if _devops_version_ge "$apt_ver" "$min_version"; then
                return 0
            fi
        fi
    fi

    local arch_name tag ver asset url tmp extract
    arch_name=$(_devops_github_linux_arch)
    [[ -n "$arch_name" ]] || { warn "fzf: 不支持的架构 $(uname -m)"; return 1; }

    tag=$(get_latest_github_release "junegunn/fzf")
    [[ "$tag" =~ ^v?[0-9] ]] || tag="v0.73.1"
    ver="${tag#v}"
    asset="fzf-${ver}-linux_${arch_name}.tar.gz"
    url="https://github.com/junegunn/fzf/releases/download/${tag}/${asset}"
    tmp="/tmp/fzf_${ver}_${arch_name}.tar.gz"
    extract="/tmp/fzf_extract_${ver}"

    if download_with_fallback "$tmp" "$url"; then
        rm -rf "$extract"
        mkdir -p "$extract"
        if tar -xzf "$tmp" -C "$extract"; then
            local bin_fzf
            bin_fzf=$(find "$extract" -type f -name "fzf" -executable | head -n1)
            if [[ -n "$bin_fzf" ]]; then
                install -m 0755 "$bin_fzf" /usr/local/bin/fzf
            fi
        fi
        rm -rf "$tmp" "$extract"
    fi

    command -v fzf >/dev/null 2>&1 || warn "fzf 安装失败，Fish / Micro / Yazi 模糊搜索可能不可用"
}

_ensure_devops_fd() {
    if command -v fd >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available fd-find; then
        safe_apt_install fd-find || true
    fi
    if command -v fdfind >/dev/null 2>&1; then
        ln -sf "$(command -v fdfind)" /usr/local/bin/fd
        return 0
    fi

    local target tag asset url tmp extract bin_fd
    case "$(uname -m)" in
        x86_64)  target="x86_64-unknown-linux-gnu" ;;
        aarch64) target="aarch64-unknown-linux-gnu" ;;
        armv7l|armhf) target="armv7-unknown-linux-gnueabihf" ;;
        i386|i686) target="i686-unknown-linux-gnu" ;;
        *) warn "fd: 不支持的架构 $(uname -m)"; return 1 ;;
    esac

    tag=$(get_latest_github_release "sharkdp/fd")
    [[ "$tag" =~ ^v?[0-9] ]] || tag="v10.2.0"
    asset="fd-${tag}-${target}.tar.gz"
    url="https://github.com/sharkdp/fd/releases/download/${tag}/${asset}"
    tmp="/tmp/fd_${tag}_${target}.tar.gz"
    extract="/tmp/fd_extract_${tag}"

    if download_with_fallback "$tmp" "$url"; then
        rm -rf "$extract"
        mkdir -p "$extract"
        if tar -xzf "$tmp" -C "$extract"; then
            bin_fd=$(find "$extract" -type f -name "fd" -executable | head -n1)
            if [[ -n "$bin_fd" ]]; then
                install -m 0755 "$bin_fd" /usr/local/bin/fd
            fi
        fi
        rm -rf "$tmp" "$extract"
    fi

    command -v fd >/dev/null 2>&1 || warn "fd 安装失败，Yazi 文件搜索可能不可用"
}

_ensure_devops_ripgrep() {
    if command -v rg >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available ripgrep; then
        safe_apt_install ripgrep || true
        command -v rg >/dev/null 2>&1 && return 0
    fi

    local dpkg_arch tag ver deb_name url tmp
    dpkg_arch=$(_devops_github_dpkg_arch)
    [[ -n "$dpkg_arch" ]] || { warn "ripgrep: 不支持的架构 $(uname -m)"; return 1; }

    tag=$(get_latest_github_release "BurntSushi/ripgrep")
    [[ "$tag" =~ ^v?[0-9] ]] || tag="v14.1.1"
    ver="${tag#v}"
    deb_name="ripgrep_${ver}-1_${dpkg_arch}.deb"
    url="https://github.com/BurntSushi/ripgrep/releases/download/${tag}/${deb_name}"
    tmp="/tmp/ripgrep_${ver}_${dpkg_arch}.deb"

    if download_with_fallback "$tmp" "$url"; then
        dpkg -i "$tmp" >/dev/null 2>&1 || apt-get install -f -yq >/dev/null 2>&1 || true
        rm -f "$tmp"
    fi

    command -v rg >/dev/null 2>&1 || warn "ripgrep 安装失败，Micro / Yazi 内容搜索可能不可用"
}

_ensure_devops_zoxide() {
    if command -v zoxide >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available zoxide; then
        safe_apt_install zoxide || true
        command -v zoxide >/dev/null 2>&1 && return 0
    fi

    info "正在通过 zoxide 官方脚本安装..."
    local tmp_zoxide="/tmp/zoxide_install.sh"
    if download_with_fallback "$tmp_zoxide" "https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh"; then
        sh "$tmp_zoxide" -y >/dev/null 2>&1 || true
        rm -f "$tmp_zoxide"
    fi

    command -v zoxide >/dev/null 2>&1 || warn "zoxide 安装失败，Fish / Yazi 历史目录导航可能不可用"
}

_ensure_devops_clipboard() {
    if command -v xclip >/dev/null 2>&1 || command -v wl-copy >/dev/null 2>&1 || command -v xsel >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available xclip; then
        safe_apt_install xclip || true
    fi
    if ! command -v xclip >/dev/null 2>&1 && _apt_pkg_available wl-clipboard; then
        safe_apt_install wl-clipboard || true
    fi
    if ! command -v xclip >/dev/null 2>&1 && ! command -v wl-copy >/dev/null 2>&1 && _apt_pkg_available xsel; then
        safe_apt_install xsel || true
    fi

    if ! command -v xclip >/dev/null 2>&1 && ! command -v wl-copy >/dev/null 2>&1 && ! command -v xsel >/dev/null 2>&1; then
        warn "剪贴板工具（xclip / wl-clipboard / xsel）均未安装，终端剪贴板共享可能不可用"
    fi
}

_ensure_devops_bat() {
    if command -v bat >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available bat; then
        safe_apt_install bat || true
    fi
    if command -v batcat >/dev/null 2>&1; then
        ln -sf "$(command -v batcat)" /usr/local/bin/bat
    fi

    command -v bat >/dev/null 2>&1 || warn "bat 安装失败，MicroOmni 语法高亮预览可能不可用"
}

_ensure_devops_0xproto_nerd_font() {
    safe_apt_install fontconfig || true

    local font_dir="/usr/local/share/fonts/0xProto-Nerd-Font"
    local marker="$font_dir/.debopti-installed"
    if [[ -f "$marker" ]] && fc-list 2>/dev/null | grep -qi "0xProtoNerdFont"; then
        return 0
    fi

    local pkg installed_via_apt=false
    for pkg in fonts-0xproto-nerd-font fonts-0xproto-nerd-font-mono fonts-0xproto-nerd-font-propo; do
        if _apt_pkg_available "$pkg"; then
            safe_apt_install "$pkg" && installed_via_apt=true
        fi
    done
    if [[ "$installed_via_apt" == "true" ]] && fc-list 2>/dev/null | grep -qi "0xProtoNerdFont"; then
        touch "$marker"
        fc-cache -f >/dev/null 2>&1 || true
        return 0
    fi

    # 解压 .tar.xz 需要 xz-utils；.zip 需要 unzip（与 Yazi 共用，此处幂等安装）
    if _apt_pkg_available xz-utils; then
        safe_apt_install xz-utils || true
    fi
    if _apt_pkg_available unzip; then
        safe_apt_install unzip || true
    fi

    local tag tmp_xz tmp_zip extract extracted=false
    tag=$(get_latest_github_release "ryanoasis/nerd-fonts")
    [[ "$tag" =~ ^v?[0-9] ]] || tag="v3.4.0"
    tmp_xz="/tmp/0xProto-nerd-font.tar.xz"
    tmp_zip="/tmp/0xProto-nerd-font.zip"
    extract="/tmp/0xProto-nerd-font-extract"

    info "正在从 Nerd Fonts 官方 Release 安装 0xProto（NF / Mono / Propo）..."
    rm -rf "$extract"
    mkdir -p "$extract" "$font_dir"

    if command -v xz >/dev/null 2>&1; then
        local url_xz="https://github.com/ryanoasis/nerd-fonts/releases/download/${tag}/0xProto.tar.xz"
        if download_with_fallback "$tmp_xz" "$url_xz" && tar -xJf "$tmp_xz" -C "$extract"; then
            extracted=true
        fi
    fi

    if [[ "$extracted" != "true" ]] && command -v unzip >/dev/null 2>&1; then
        rm -rf "$extract"
        mkdir -p "$extract"
        local url_zip="https://github.com/ryanoasis/nerd-fonts/releases/download/${tag}/0xProto.zip"
        if download_with_fallback "$tmp_zip" "$url_zip" && unzip -q -o "$tmp_zip" -d "$extract"; then
            extracted=true
        fi
    fi

    if [[ "$extracted" == "true" ]]; then
        find "$extract" -type f \( -iname '*.ttf' -o -iname '*.otf' \) -exec cp -f {} "$font_dir/" \;
        if compgen -G "${font_dir}/*" >/dev/null; then
            touch "$marker"
            fc-cache -f "$font_dir" >/dev/null 2>&1 || fc-cache -f >/dev/null 2>&1 || true
        else
            warn "0xProto Nerd Font 解压后未找到字体文件"
        fi
    else
        warn "0xProto Nerd Font 下载或解压失败（需 xz-utils 或 unzip）；请在 SSH 客户端选用 Nerd Font"
    fi

    rm -f "$tmp_xz" "$tmp_zip"
    rm -rf "$extract"
}

_ensure_devops_resvg() {
    if command -v resvg >/dev/null 2>&1; then
        return 0
    fi

    if _apt_pkg_available resvg; then
        safe_apt_install resvg || true
        command -v resvg >/dev/null 2>&1 && return 0
    fi

    if [[ "$(uname -m)" != "x86_64" ]]; then
        warn "resvg: 当前架构 $(uname -m) 无官方预编译包且 apt 源不可用，SVG 预览可能不可用"
        return 1
    fi

    local tag url tmp extract bin_resvg
    tag=$(get_latest_github_release "linebender/resvg")
    [[ "$tag" =~ ^v?[0-9] ]] || tag="v0.47.0"
    url="https://github.com/linebender/resvg/releases/download/${tag}/resvg-linux-x86_64.tar.gz"
    tmp="/tmp/resvg_${tag#v}_linux_x86_64.tar.gz"
    extract="/tmp/resvg_extract_${tag#v}"

    if download_with_fallback "$tmp" "$url"; then
        rm -rf "$extract"
        mkdir -p "$extract"
        if tar -xzf "$tmp" -C "$extract"; then
            bin_resvg=$(find "$extract" -type f -name "resvg" -executable | head -n1)
            if [[ -n "$bin_resvg" ]]; then
                install -m 0755 "$bin_resvg" /usr/local/bin/resvg
            fi
        fi
        rm -rf "$tmp" "$extract"
    fi

    command -v resvg >/dev/null 2>&1 || warn "resvg 安装失败，SVG 预览可能不可用"
}

# Fish / Micro / Yazi 重合 CLI 依赖的统一安装入口
_install_devops_shared_cli_deps() {
    apt-get update -yq >/dev/null 2>&1 || true
    _ensure_devops_fzf
    _ensure_devops_fd
    _ensure_devops_ripgrep
    _ensure_devops_zoxide
    _ensure_devops_clipboard
    _ensure_devops_bat
    _ensure_devops_0xproto_nerd_font
}

# ----------------- Fish Shell 安装与增强 -----------------
install_fish() {
    info "正在安装 Fish Shell 及其生态工具..."
    
    # 1. 安装本体与共享 CLI 依赖
    safe_apt_install fish curl git
    _install_devops_shared_cli_deps

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

    # 1.2.1 puffer-fish 兼容：Fish 3.x 无 commandline --search-field，覆盖 fisher 安装的函数
    render_template "templates/apps/devops/puffer_fish_expand_dot.fish" \
        "$functions_dir/_puffer_fish_expand_dot.fish"
    chown "$sot_user:$sot_user" "$functions_dir/_puffer_fish_expand_dot.fish" 2>/dev/null || true
    
    # 1.3 配置文件加载 (SOT 物理环境)
    render_template "templates/apps/devops/zoxide.fish" "$conf_d_dir/zoxide.fish"
    render_template "templates/apps/devops/starship.fish" "$conf_d_dir/starship.fish"
    
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

    # 2. 设置默认 Shell 为 Fish (对包括 root 在内的所有真实用户生效)
    for u in "${all_users[@]}"; do
        info "正在将用户 $u 的默认 Shell 设置为 Fish..."
        local fish_path=$(which fish 2>/dev/null || echo "/usr/bin/fish")
        chsh -s "$fish_path" "$u" 2>/dev/null || true
    done
    
    deploy_fish_yazi_wrapper
    sync_devops_sot_links

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
    
    # 3. 清除各用户 Fish / Starship 软链（不跟随软链删除 SOT 物理目录）
    local sot_user
    sot_user=$(get_sot_user)
    for user in "${all_users[@]}"; do
        local user_home fish_dir item
        user_home=$(eval echo "~$user")
        fish_dir="$user_home/.config/fish"
        if [[ "$user" == "$sot_user" ]]; then
            rm -rf "$fish_dir"
        elif [[ -L "$fish_dir" ]]; then
            rm -f "$fish_dir"
        elif [[ -d "$fish_dir" ]]; then
            for item in "$fish_dir"/*; do
                [[ -e "$item" ]] || continue
                if [[ -L "$item" ]]; then
                    rm -f "$item"
                else
                    rm -rf "$item"
                fi
            done
            rmdir "$fish_dir" 2>/dev/null || true
        fi
        rm -f "$user_home/.config/starship.toml"
    done
    rm -rf /etc/fish/shared_sot
    rm -f /etc/starship.toml
    rm -f "$SOT_KNOWN_USERS_FILE"

    success "Fish 及其配置已彻底清理。"
}

# MicroOmni Session.lua：修正 upstream ff28759e 将 goos.MkdirAll 误写为 os.MkdirAll
_patch_micro_omni_session_lua() {
    local session_lua="$1"
    local sed_rules="${SCRIPT_DIR:-/opt/debopti}/templates/apps/devops/micro_omni_session.sed"
    [[ -f "$session_lua" ]] || return 0
    [[ -f "$sed_rules" ]] || {
        warn "MicroOmni 兼容补丁模板未找到: $sed_rules"
        return 1
    }

    mkdir -p "$(dirname "$session_lua")/sessions"

    # 勿用全局 s/os\.ModePerm/goos.ModePerm/：会误伤 goos.* 变成 gogoos / gogogoos
    if grep -qE 'gogo+os\.' "$session_lua" 2>/dev/null; then
        sed -i 's/gogogoos/goos/g; s/gogoos/goos/g' "$session_lua"
        info "已修复 MicroOmni Session.lua 中被 broad sed 损坏的 goos 变量名"
    fi

    if grep -q 'os\.MkdirAll(sessionsPath, os\.ModePerm)' "$session_lua" 2>/dev/null; then
        sed -i -f "$sed_rules" "$session_lua"
        info "已修补 MicroOmni Session.lua（goos.MkdirAll 会话目录创建）"
    fi
}

# ----------------- Micro 编辑器安装 -----------------
install_micro() {
    info "正在安装 Micro 编辑器 最新二进制版..."
    safe_apt_install shellcheck yamllint
    _install_devops_shared_cli_deps

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

    _patch_micro_omni_session_lua "$plug_dir/MicroOmni/Session.lua"

    chown -R "$sot_user:$sot_user" "$sot_home/.config/micro" 2>/dev/null || true

    # 3. 注册全局环境变量与 Fish 配置
    local profile_file="/etc/profile.d/micro_env.sh"
    render_template "templates/apps/devops/micro_env.sh" "$profile_file"
    chmod +x "$profile_file"
    set_system_env "EDITOR" "micro"
    set_system_env "VISUAL" "micro"
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

    sync_devops_sot_links
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
    
    local sot_user all_users user user_home
    sot_user=$(get_sot_user)
    all_users=($(get_all_real_users))
    for user in "${all_users[@]}"; do
        user_home=$(eval echo "~$user")
        if [[ "$user" == "$sot_user" ]]; then
            rm -rf "$user_home/.config/micro"
        elif [[ -L "$user_home/.config/micro" ]]; then
            rm -f "$user_home/.config/micro"
        else
            rm -rf "$user_home/.config/micro"
        fi
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

_lego_escape_env_value() {
    printf '%s' "$1" | sed 's/"/\\"/g'
}

_lego_is_safe_primary_domain() {
    local d="${1:-}"
    [[ -n "$d" && "$d" != *"*"* && "$d" != *"/"* && "$d" != *".."* \
        && "$d" != *'"'* && "$d" != *"'"* && "$d" != *" "* \
        && "$d" != *$'\n'* && "$d" != *$'\r'* && "$d" != *";"* ]]
}

_lego_is_safe_domain_entry() {
    local d="${1:-}"
    [[ -n "$d" && "$d" != *"/"* && "$d" != *".."* \
        && "$d" != *'"'* && "$d" != *"'"* && "$d" != *" "* \
        && "$d" != *$'\n'* && "$d" != *$'\r'* && "$d" != *";"* ]]
}

_lego_run_hook() {
    local domain="${1:-}"
    if [[ $EUID -eq 0 ]]; then
        /usr/local/bin/debopti-lego-hook.sh "$domain"
    elif command -v sudo >/dev/null 2>&1; then
        sudo /usr/local/bin/debopti-lego-hook.sh "$domain"
    else
        err "需要 root 权限推送证书至 Ferron。"
        return 1
    fi
}

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
    mkdir -p /etc/lego/envs /var/lib/lego/accounts /var/lib/lego/certificates
    chmod 700 /var/lib/lego /var/lib/lego/accounts /var/lib/lego/certificates /etc/lego/envs 2>/dev/null || true

    # 通过模板引擎部署自动更新相关的脚本与 Systemd 任务
    info "部署自动续期脚本与定时任务..."
    render_template "templates/apps/lego/debopti-lego-lib.sh" "/usr/local/bin/debopti-lego-lib.sh"
    render_template "templates/apps/lego/debopti-lego-run-once.sh" "/usr/local/bin/debopti-lego-run-once.sh"
    render_template "templates/apps/lego/debopti-lego-renew.sh" "/usr/local/bin/debopti-lego-renew.sh"
    render_template "templates/apps/lego/debopti-lego-hook.sh" "/usr/local/bin/debopti-lego-hook.sh"
    chmod +x /usr/local/bin/debopti-lego-run-once.sh /usr/local/bin/debopti-lego-renew.sh /usr/local/bin/debopti-lego-hook.sh

    render_template "templates/apps/lego/debopti-lego-renew.service" "/etc/systemd/system/debopti-lego-renew.service"
    render_template "templates/apps/lego/debopti-lego-renew.timer" "/etc/systemd/system/debopti-lego-renew.timer"

    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable --now debopti-lego-renew.timer >/dev/null 2>&1 || true

    # v4 数据目录升级至 v5 布局（幂等，已迁移则跳过）
    if [[ -d /var/lib/lego ]] && /usr/local/bin/lego migrate --help >/dev/null 2>&1; then
        info "检查 Lego 数据目录是否需要迁移（v4 → v5）..."
        if ! /usr/local/bin/lego migrate --path=/var/lib/lego 2>/dev/null; then
            warn "Lego 数据迁移跳过或无需迁移。"
        fi
    fi

    success "Lego 部署及自动续期定时任务配置完成。"
}

uninstall_lego() {
    info "正在移除 Lego 及其自动化托管资源..."
    
    # 停止并禁用定时器
    systemctl disable --now debopti-lego-renew.timer >/dev/null 2>&1 || true
    systemctl stop debopti-lego-renew.service >/dev/null 2>&1 || true
    
    # 物理清理二进制及脚本
    rm -f /usr/local/bin/lego
    rm -f /usr/local/bin/debopti-lego-lib.sh
    rm -f /usr/local/bin/debopti-lego-run-once.sh
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
                    status_text="${YELLOW}未申请${NC}"
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
        elif ! _lego_is_safe_primary_domain "$primary_domain"; then
            warn "主域名格式无效（不可含 /、*、空格或引号）。"
            primary_domain=""
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
        elif [[ "$email" != *"@"* || "$email" == *'"'* ]]; then
            warn "邮箱格式无效。"
            email=""
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
        local sub_part="" d trimmed
        IFS=',' read -ra SUB_ADDR <<< "$sub_domains"
        for d in "${SUB_ADDR[@]}"; do
            trimmed="${d#"${d%%[![:space:]]*}"}"
            trimmed="${trimmed%"${trimmed##*[![:space:]]}"}"
            [[ -z "$trimmed" ]] && continue
            if ! _lego_is_safe_domain_entry "$trimmed"; then
                err "备用域名格式无效: $trimmed"
                pause
                return 0
            fi
            sub_part="${sub_part:+$sub_part,}$trimmed"
        done
        if [[ -n "$sub_part" ]]; then
            domains="${primary_domain},${sub_part}"
        fi
    fi

    local env_file="/etc/lego/envs/${primary_domain}.env"
    if [[ -f "$env_file" ]]; then
        err "域名 $primary_domain 已存在，请在列表中管理。"
        pause
        return 0
    fi

    mkdir -p "/etc/lego/envs"
    chmod 700 "/etc/lego/envs" 2>/dev/null || true

    local safe_token safe_email safe_domains
    safe_token=$(_lego_escape_env_value "$cf_token")
    safe_email=$(_lego_escape_env_value "$email")
    safe_domains=$(_lego_escape_env_value "$domains")

    render_template "templates/apps/lego/lego.env" "$env_file" \
        "CF_TOKEN=$safe_token" \
        "DOMAINS=$safe_domains" \
        "EMAIL=$safe_email" \
        "PROVIDER=cloudflare" \
        "AUTO_RENEW=true" \
        "FERRON_PUSH=false"

    chmod 600 "$env_file" 2>/dev/null || true
    chown root:root "$env_file" 2>/dev/null || true

    success "域名配置已保存到 $env_file"
    echo ""

    if [[ ! -x /usr/local/bin/debopti-lego-run-once.sh ]]; then
        warn "未找到证书申请脚本，请重新安装 Lego 后，在域名详情中手动申请。"
        pause
        return 0
    fi

    info "正在自动申请首次证书（将通过 sudo 写入 /var/lib/lego）..."
    echo ""
    if /usr/local/bin/debopti-lego-run-once.sh "$env_file" issue; then
        success "首次证书申请成功！"
    else
        err "首次证书申请失败。请检查 Cloudflare API Token、域名解析与网络后，在域名详情中选择「立即申请/续期」重试。"
    fi

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
            status_text="${YELLOW}未申请${NC}"
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
        ui_draw_item "4" "📋 查看手动申请命令（故障排查）"
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
                            _lego_run_hook "$primary_domain" || true
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
                echo -e " 故障排查时可手动执行："
                echo -e " ------------------------------------------------------------"
                echo -e " ${YELLOW}sudo /usr/local/bin/debopti-lego-run-once.sh \"${env_file}\" issue${NC}"
                echo -e " ------------------------------------------------------------"
                echo -e " 等价的底层 lego 命令："
                echo -e " ------------------------------------------------------------"
                echo -e " ${YELLOW}sudo /usr/local/bin/lego run \\"
                echo -e "      --env-file=\"${env_file}\" \\"
                echo -e "      --email=\"${email}\" \\"
                echo -e "      --dns=\"${provider}\" \\"
                IFS=',' read -ra ADDR <<< "$domains"
                for d in "${ADDR[@]}"; do
                    echo -e "      --domains=\"$d\" \\"
                done
                echo -e "      --path=\"/var/lib/lego\" \\"
                echo -e "      --accept-tos${NC}"
                echo -e " ------------------------------------------------------------"
                pause
                ;;
            5)
                info "正在启动 Lego 申请/续期..."
                if [[ -x /usr/local/bin/debopti-lego-run-once.sh ]]; then
                    /usr/local/bin/debopti-lego-run-once.sh "$env_file" auto || true
                else
                    err "未找到 /usr/local/bin/debopti-lego-run-once.sh，请重新安装 Lego。"
                fi
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

# 将 Yazi 的 Fish 包装命令 `y` 部署到 SOT functions/ 与系统级 conf.d
deploy_fish_yazi_wrapper() {
    [[ -f /usr/local/bin/yazi ]] || return 0
    if ! command -v fish >/dev/null 2>&1 && [[ ! -d /etc/fish ]]; then
        return 0
    fi

    local sot_user sot_home
    sot_user=$(get_sot_user)
    sot_home=$(eval echo "~$sot_user")

    mkdir -p /etc/fish/conf.d
    render_template "templates/apps/devops/y.fish" "/etc/fish/conf.d/yazi.fish"
    chmod a+r /etc/fish/conf.d/yazi.fish 2>/dev/null || true

    if [[ -d "$sot_home/.config/fish" ]]; then
        _migrate_sot_fish_from_shared_sot "$sot_user" "$sot_home"
        mkdir -p "$sot_home/.config/fish/functions"
        render_template "templates/apps/devops/y.fish" "$sot_home/.config/fish/functions/y.fish"
        chown "$sot_user:$sot_user" "$sot_home/.config/fish/functions/y.fish" 2>/dev/null || true
        apply_sot_shared_readonly "$sot_home/.config/fish" "$sot_user"
    fi
}

# 安装 Yazi 官方文档推荐的可选 CLI 依赖
# 多媒体预览包（ffmpeg / poppler / resvg / ImageMagick）整组选装，选择持久化到 debopti.conf
_install_yazi_dependencies() {
    load_project_config

    info "正在安装 Yazi 官方推荐 CLI 依赖..."

    safe_apt_install unzip file jq p7zip-full
    _install_devops_shared_cli_deps

    local install_media="${YAZI_MEDIA_PREVIEW:-}"
    if [[ -z "$install_media" ]]; then
        local media_choice="n"
        if [[ -t 0 ]]; then
            read -r -p "是否安装多媒体预览依赖（ffmpeg、poppler、resvg、ImageMagick）？[y/N]: " media_choice
        fi
        if [[ "${media_choice:-n}" == "y" || "${media_choice:-n}" == "Y" ]]; then
            install_media="true"
        else
            install_media="false"
        fi
        save_project_config "YAZI_MEDIA_PREVIEW" "$install_media"
        YAZI_MEDIA_PREVIEW="$install_media"
    fi

    if [[ "$install_media" == "true" ]]; then
        info "正在安装多媒体预览依赖（ffmpeg / poppler / resvg / ImageMagick）..."
        safe_apt_install ffmpeg poppler-utils imagemagick || warn "部分多媒体依赖安装失败，相关预览可能不可用"
        _ensure_devops_resvg
    else
        info "已跳过多媒体预览依赖（ffmpeg / poppler / resvg / ImageMagick）"
    fi
}

# ----------------- Yazi 文件管理器安装 -----------------
install_yazi() {
    info "正在安装 Yazi 极速终端文件管理器..."
    _install_yazi_dependencies

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
    render_template "templates/apps/devops/yazi_init.lua" "$sot_yazi_conf/init.lua"
    
    # 5. 全局及多用户注册 Fish Shell wrapper
    deploy_fish_yazi_wrapper

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

    sync_devops_sot_links

    success "Yazi 文件管理器安装配置完成。您可以在终端中直接输入 'y' 启动以获得自动 CWD 同步支持。"
}

uninstall_yazi() {
    info "正在移除 Yazi 文件管理器..."
    
    # 1. 物理清理二进制程序与全局包装器
    rm -f /usr/local/bin/yazi
    rm -f /usr/local/bin/ya
    rm -f /etc/profile.d/yazi_wrapper.sh
    rm -f /etc/fish/conf.d/yazi.fish

    local sot_user sot_home
    sot_user=$(get_sot_user)
    sot_home=$(eval echo "~$sot_user")
    rm -f "$sot_home/.config/fish/functions/y.fish"

    # 清理 /etc/bash.bashrc 中的注入
    if [[ -f /etc/bash.bashrc ]]; then
        sed -i '/# Yazi CWD Sync Wrapper/,+3d' /etc/bash.bashrc
    fi

    # 2. 清理所有用户的配置文件
    local all_users=($(get_all_real_users))
    for user in "${all_users[@]}"; do
        local user_home
        user_home=$(eval echo "~$user")
        if [[ "$user" == "$sot_user" ]]; then
            rm -rf "$user_home/.config/yazi"
        elif [[ -L "$user_home/.config/yazi" ]]; then
            rm -f "$user_home/.config/yazi"
        else
            rm -rf "$user_home/.config/yazi"
        fi
    done

    success "Yazi 已彻底从系统移除。"
}
