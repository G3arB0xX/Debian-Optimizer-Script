#!/bin/bash
# =========================================================
# Podman Rootless 容器环境部署模块
# =========================================================
# 架构: apps 专用用户 rootless 运行，主用户通过 apppod/appctl/applog 代管
# =========================================================

PODMAN_APPS_USER="apps"
PODMAN_APPS_HOME="/home/${PODMAN_APPS_USER}"
PODMAN_DATA_DIR="${PODMAN_APPS_HOME}/srv"
PODMAN_STORAGE_DIR="${PODMAN_DATA_DIR}/storage"
PODMAN_DEBUG_LOG="${SCRIPT_DIR:-/opt/debopti}/.cursor/debug-99e7f0.log"

# ----------------- 内部工具 -----------------

# #region agent log
_podman_debug_log() {
    local hypo="$1" loc="$2" msg="$3" data="${4:-{}}"
    mkdir -p "$(dirname "${PODMAN_DEBUG_LOG}")" 2>/dev/null || true
    printf '{"sessionId":"99e7f0","hypothesisId":"%s","location":"%s","message":"%s","data":%s,"timestamp":%s}\n' \
        "$hypo" "$loc" "$msg" "$data" "$(date +%s%3N 2>/dev/null || date +%s)" >> "${PODMAN_DEBUG_LOG}" 2>/dev/null || true
}
# #endregion

_podman_get_apps_uid() {
    id -u "${PODMAN_APPS_USER}" 2>/dev/null || echo ""
}

_podman_apps_runtime() {
    local uid
    uid=$(_podman_get_apps_uid)
    [[ -n "$uid" ]] && echo "/run/user/${uid}" || echo ""
}

# 引导 apps 用户 systemd 会话（创建 /run/user/UID 运行时目录）
_podman_bootstrap_apps_session() {
    local uid runtime
    uid=$(_podman_get_apps_uid)
    [[ -z "$uid" ]] && return 1
    runtime="/run/user/${uid}"

    loginctl enable-linger "${PODMAN_APPS_USER}" 2>/dev/null || true
    systemctl daemon-reload 2>/dev/null || true
    systemctl start "user@${uid}.service" 2>/dev/null || true

    local i=0
    while [[ ! -d "$runtime" && "$i" -lt 10 ]]; do
        sleep 0.3
        systemctl start "user@${uid}.service" 2>/dev/null || true
        i=$((i + 1))
    done

    if [[ ! -d "$runtime" ]]; then
        mkdir -p /run/user 2>/dev/null || true
        if ! install -d -m 0700 -o "${PODMAN_APPS_USER}" -g "${PODMAN_APPS_USER}" "$runtime" 2>/dev/null; then
            mkdir -p "$runtime" 2>/dev/null || true
            chown "${PODMAN_APPS_USER}:${PODMAN_APPS_USER}" "$runtime" 2>/dev/null || true
            chmod 0700 "$runtime" 2>/dev/null || true
        fi
    fi

    if [[ -d "$runtime" && ! -S "${runtime}/bus" ]]; then
        safe_apt_install dbus-user-session 2>/dev/null || true
        sudo -u "${PODMAN_APPS_USER}" \
            dbus-daemon --session --address="unix:path=${runtime}/bus" --fork 2>/dev/null || true
    fi

    # #region agent log
    _podman_debug_log "H2" "podman.sh:_podman_bootstrap_apps_session" "bootstrap_done" \
        "{\"uid\":${uid},\"runtime_exists\":$([ -d \"$runtime\" ] && echo true || echo false)}"
    # #endregion

    [[ -d "$runtime" ]]
}

# 以 apps 用户执行命令（自动注入 XDG_RUNTIME_DIR）
_podman_apps_exec() {
    local uid runtime
    uid=$(_podman_get_apps_uid)
    [[ -z "$uid" ]] && return 1
    runtime="/run/user/${uid}"
    _podman_bootstrap_apps_session 2>/dev/null || true
    sudo -u "${PODMAN_APPS_USER}" XDG_RUNTIME_DIR="${runtime}" "$@"
}

_podman_maintainer_user() {
    local user
    user=$(get_initial_user 2>/dev/null || echo "")
    [[ -z "$user" || "$user" == "root" ]] && user=$(get_sot_user 2>/dev/null || echo "root")
    echo "$user"
}

# 检测 Podman 主版本是否 >= 5（sqlite 后端需要）
_podman_supports_sqlite() {
    local ver major
    ver=$(podman version --format '{{.Version}}' 2>/dev/null || podman --version 2>/dev/null | awk '{print $3}')
    major=$(echo "$ver" | cut -d. -f1)
    [[ -n "$major" && "$major" -ge 5 ]]
}

# 部署 Bash / Fish 运维别名
_podman_deploy_shell_helpers() {
    local maintainer sot_user sot_home fish_conf_d

    maintainer=$(_podman_maintainer_user)
    sot_user=$(get_sot_user 2>/dev/null || echo "$maintainer")
    render_template "templates/apps/podman/profile.d/debopti-podman.sh" "/etc/profile.d/debopti-podman.sh" \
        "SOT_USER=$sot_user"
    chmod 644 /etc/profile.d/debopti-podman.sh

    if ! command -v fish >/dev/null 2>&1; then
        return 0
    fi

    sot_home=$(eval echo "~$sot_user")
    fish_conf_d="$sot_home/.config/fish/conf.d"
    mkdir -p "$fish_conf_d"
    chown -R "$sot_user:$sot_user" "$sot_home/.config/fish" 2>/dev/null || true

    local fish_tpl
    for fish_tpl in podman_env podman_abbr; do
        render_template "templates/apps/podman/fish/${fish_tpl}.fish" "${fish_conf_d}/debopti_${fish_tpl}.fish"
        chown "$sot_user:$sot_user" "${fish_conf_d}/debopti_${fish_tpl}.fish" 2>/dev/null || true
        chmod o+r "${fish_conf_d}/debopti_${fish_tpl}.fish" 2>/dev/null || true
    done
    for fish_tpl in apppod appctl applog appshell; do
        render_template "templates/apps/podman/fish/${fish_tpl}.fish" "${fish_conf_d}/debopti_${fish_tpl}.fish"
        chown "$sot_user:$sot_user" "${fish_conf_d}/debopti_${fish_tpl}.fish" 2>/dev/null || true
        chmod o+r "${fish_conf_d}/debopti_${fish_tpl}.fish" 2>/dev/null || true
    done

    if declare -f sync_devops_sot_links > /dev/null; then
        sync_devops_sot_links true
    fi
}

# 移除 Shell 运维别名
_podman_remove_shell_helpers() {
    rm -f /etc/profile.d/debopti-podman.sh

    local sot_user sot_home fish_conf_d fish_tpl
    sot_user=$(get_sot_user 2>/dev/null || echo "")
    [[ -z "$sot_user" ]] && return 0
    sot_home=$(eval echo "~$sot_user")
    fish_conf_d="$sot_home/.config/fish/conf.d"

    for fish_tpl in podman_env podman_abbr apppod appctl applog appshell; do
        rm -f "${fish_conf_d}/debopti_${fish_tpl}.fish"
    done
    if declare -f sync_devops_sot_links > /dev/null; then
        sync_devops_sot_links true
    fi
}

# 强制移除已安装的 Docker（避免 socket / 端口冲突）
_podman_remove_docker() {
    if ! command -v docker >/dev/null 2>&1 && ! dpkg -s docker-ce >/dev/null 2>&1; then
        return 0
    fi

    warn "检测到 Docker 环境，将自动卸载以避免与 Podman 冲突..."
    systemctl stop docker docker.socket containerd >/dev/null 2>&1 || true
    apt-get purge -yq docker-ce docker-ce-cli containerd.io \
        docker-buildx-plugin docker-compose-plugin docker.io docker-doc \
        2>/dev/null || true
    apt-get autoremove -yq >/dev/null 2>&1 || true
    rm -rf /etc/docker /var/run/docker.sock /usr/local/bin/docker-compose
    success "Docker 已移除。"
}

# 将 apps 加入 debopti-certs，以便读取 /etc/ferron/certs（与 Lego/Ferron 联动）
_podman_join_debopti_certs_group() {
    local lego_lib=""
    if [[ -n "${SCRIPT_DIR:-}" && -f "${SCRIPT_DIR}/templates/apps/lego/debopti-lego-lib.sh" ]]; then
        lego_lib="${SCRIPT_DIR}/templates/apps/lego/debopti-lego-lib.sh"
    elif [[ -f /usr/local/bin/debopti-lego-lib.sh ]]; then
        lego_lib="/usr/local/bin/debopti-lego-lib.sh"
    fi
    [[ -f "$lego_lib" ]] || return 0

    # shellcheck source=/dev/null
    source "$lego_lib"

    local g=""
    g=$(_debopti_certs_group) || return 0

    if _debopti_certs_add_member "${PODMAN_APPS_USER}" "$g"; then
        loginctl terminate-user "${PODMAN_APPS_USER}" 2>/dev/null || true
        _podman_bootstrap_apps_session 2>/dev/null || true
        info "已将 ${PODMAN_APPS_USER} 加入 ${g} 组（可读 TLS 证书目录）。"
    fi
}

# 创建 apps 专用用户
_podman_create_apps_user() {
    local apps_shell maintainer

    if command -v fish >/dev/null 2>&1; then
        apps_shell=$(command -v fish)
    else
        apps_shell="/bin/bash"
    fi

    if ! id "${PODMAN_APPS_USER}" >/dev/null 2>&1; then
        info "正在创建容器专用用户 ${PODMAN_APPS_USER}（Shell: ${apps_shell}）..."
        useradd -m -s "${apps_shell}" -d "${PODMAN_APPS_HOME}" "${PODMAN_APPS_USER}" || return 1
    else
        info "用户 ${PODMAN_APPS_USER} 已存在，更新 Shell 为 ${apps_shell}..."
        usermod -s "${apps_shell}" "${PODMAN_APPS_USER}" 2>/dev/null || true
    fi

    passwd -l "${PODMAN_APPS_USER}" 2>/dev/null || true

    # subuid/subgid：幂等追加
    if ! grep -q "^${PODMAN_APPS_USER}:" /etc/subuid 2>/dev/null; then
        usermod --add-subuids 100000-165535 "${PODMAN_APPS_USER}" || return 1
    fi
    if ! grep -q "^${PODMAN_APPS_USER}:" /etc/subgid 2>/dev/null; then
        usermod --add-subgids 100000-165535 "${PODMAN_APPS_USER}" || return 1
    fi

    loginctl enable-linger "${PODMAN_APPS_USER}" 2>/dev/null || true
    _podman_bootstrap_apps_session 2>/dev/null || true

    mkdir -p "${PODMAN_DATA_DIR}" "${PODMAN_STORAGE_DIR}"
    mkdir -p "${PODMAN_APPS_HOME}/.config/containers/registries.conf.d"
    mkdir -p "${PODMAN_APPS_HOME}/.config/containers/systemd"
    mkdir -p "${PODMAN_APPS_HOME}/.local/bin"

    chown -R "${PODMAN_APPS_USER}:${PODMAN_APPS_USER}" "${PODMAN_APPS_HOME}"

    maintainer=$(_podman_maintainer_user)
    if [[ -n "$maintainer" && "$maintainer" != "root" ]]; then
        usermod -aG "${PODMAN_APPS_USER}" "$maintainer" 2>/dev/null || true
        setfacl -R -m "u:${maintainer}:rwx" "${PODMAN_DATA_DIR}" "${PODMAN_APPS_HOME}/.config" 2>/dev/null || true
        setfacl -R -d -m "u:${maintainer}:rwx" "${PODMAN_DATA_DIR}" "${PODMAN_APPS_HOME}/.config" 2>/dev/null || true
        setfacl -m "u:${maintainer}:rx" "${PODMAN_APPS_HOME}" 2>/dev/null || true
    fi

    _podman_join_debopti_certs_group
}

# 渲染 apps 用户配置文件
_podman_configure_apps() {
    local apps_uid sqlite_backend fuse_section mirror_entries maintainer

    apps_uid=$(_podman_get_apps_uid)
    [[ -z "$apps_uid" ]] && return 1

    if _podman_supports_sqlite; then
        sqlite_backend='database_backend = "sqlite"'
    else
        sqlite_backend='# database_backend 需 Podman 5.x+，当前版本已跳过'
        warn "当前 Podman 版本不支持 database_backend=sqlite，已跳过该项。"
    fi

    if command -v fuse-overlayfs >/dev/null 2>&1; then
        fuse_section='[storage.options.overlay]
mount_program = "/usr/bin/fuse-overlayfs"'
    else
        fuse_section='# fuse-overlayfs 未安装，使用内核原生 overlay'
    fi

    render_template "templates/apps/podman/containers.conf" \
        "${PODMAN_APPS_HOME}/.config/containers/containers.conf" \
        "SQLITE_BACKEND=${sqlite_backend}"

    render_template "templates/apps/podman/storage.conf" \
        "${PODMAN_APPS_HOME}/.config/containers/storage.conf" \
        "APPS_UID=${apps_uid}" "FUSE_OVERLAY_SECTION=${fuse_section}"

    # #region agent log
    local _dbg_c_lines=0 _dbg_s_graph=0
    [[ -f "${PODMAN_APPS_HOME}/.config/containers/containers.conf" ]] && \
        _dbg_c_lines=$(wc -l "${PODMAN_APPS_HOME}/.config/containers/containers.conf" | awk '{print $1}')
    [[ -f "${PODMAN_APPS_HOME}/.config/containers/storage.conf" ]] && \
        _dbg_s_graph=$(grep -c graphroot "${PODMAN_APPS_HOME}/.config/containers/storage.conf" 2>/dev/null || echo 0)
    _podman_debug_log "H1" "podman.sh:_podman_configure_apps" "config_rendered" \
        "{\"apps_uid\":${apps_uid},\"sqlite\":$(_podman_supports_sqlite && echo true || echo false),\"containers_lines\":${_dbg_c_lines},\"storage_has_graphroot\":${_dbg_s_graph}}"
    # #endregion

    render_template "templates/apps/podman/registries.conf.d/000-unqualified-search.conf" \
        "${PODMAN_APPS_HOME}/.config/containers/registries.conf.d/000-unqualified-search.conf"

    if [[ "${IS_CN_REGION:-false}" == "true" ]]; then
        mirror_entries='[[registry.mirror]]
location = "docker.m.daocloud.io"

[[registry.mirror]]
location = "mirror.baidubce.com"

[[registry.mirror]]
location = "docker.nju.edu.cn"'
        render_template "templates/apps/podman/registries.conf.d/001-mirrors.conf" \
            "${PODMAN_APPS_HOME}/.config/containers/registries.conf.d/001-mirrors.conf" \
            "MIRROR_ENTRIES=${mirror_entries}"
    else
        rm -f "${PODMAN_APPS_HOME}/.config/containers/registries.conf.d/001-mirrors.conf"
    fi

    render_template "templates/apps/podman/quadlet/hello.container.example" \
        "${PODMAN_APPS_HOME}/.config/containers/systemd/hello.container.example"
    render_template "templates/apps/podman/quadlet/tls-ferron-certs.container.example" \
        "${PODMAN_APPS_HOME}/.config/containers/systemd/tls-ferron-certs.container.example"

    mkdir -p "${PODMAN_APPS_HOME}/.local/bin"
    mkdir -p "${PODMAN_APPS_HOME}/.config/systemd/user"

    render_template "templates/apps/podman/systemd-user/podman-gc.sh" \
        "${PODMAN_APPS_HOME}/.local/bin/podman-gc.sh"
    chmod 755 "${PODMAN_APPS_HOME}/.local/bin/podman-gc.sh"

    render_template "templates/apps/podman/systemd-user/podman-gc.service" \
        "${PODMAN_APPS_HOME}/.config/systemd/user/podman-gc.service"
    render_template "templates/apps/podman/systemd-user/podman-gc.timer" \
        "${PODMAN_APPS_HOME}/.config/systemd/user/podman-gc.timer"
    chown -R "${PODMAN_APPS_USER}:${PODMAN_APPS_USER}" "${PODMAN_APPS_HOME}/.config" \
        "${PODMAN_APPS_HOME}/.local" 2>/dev/null || true

    maintainer=$(_podman_maintainer_user)
    render_template "templates/apps/podman/sudoers.d/debopti-podman" \
        "/etc/sudoers.d/debopti-podman" "MAINTAINER_USER=${maintainer}"
    chmod 440 /etc/sudoers.d/debopti-podman
    if ! visudo -cf /etc/sudoers.d/debopti-podman >/dev/null 2>&1; then
        # #region agent log
        _podman_debug_log "H3" "podman.sh:_podman_configure_apps" "sudoers_validation_failed" \
            "{\"maintainer\":\"${maintainer}\",\"visudo_out\":\"$(visudo -cf /etc/sudoers.d/debopti-podman 2>&1 | tr '\n' ' ' | sed 's/"/\\"/g')\"}"
        # #endregion
        err "sudoers 配置校验失败，已回滚。"
        rm -f /etc/sudoers.d/debopti-podman
        return 1
    fi

    render_template "templates/apps/podman/sysctl/99-debopti-podman.conf" \
        "/etc/sysctl.d/99-debopti-podman.conf"
    sysctl --system >/dev/null 2>&1 || sysctl -p /etc/sysctl.d/99-debopti-podman.conf >/dev/null 2>&1 || true
}

# 按 Debian 版本安装 APT 包（旧版无 podman-docker / podman-compose）
_podman_install_packages() {
    local pkgs=(podman buildah skopeo uidmap systemd-container acl)

    # 基础镜像可能已清除 apt lists，先更新索引再探测可选包
    apt-get update -yq >/dev/null 2>&1 || true

    if apt-cache show podman-docker >/dev/null 2>&1; then
        pkgs+=(podman-docker)
    else
        warn "当前 Debian 版本无 podman-docker 包，将手动创建 docker 兼容软链。"
    fi
    if apt-cache show podman-compose >/dev/null 2>&1; then
        pkgs+=(podman-compose)
    fi

    if ! safe_apt_install "${pkgs[@]}"; then
        warn "APT 安装失败，尝试 --fix-missing 重试..."
        apt-get update -yq >/dev/null 2>&1
        apt-get install -yq --fix-missing "${pkgs[@]}" || return 1
    fi

    if ! command -v docker >/dev/null 2>&1 && command -v podman >/dev/null 2>&1; then
        ln -sf "$(command -v podman)" /usr/local/bin/docker 2>/dev/null || true
    fi

    if ! command -v podman-compose >/dev/null 2>&1; then
        info "尝试通过 pip 安装 podman-compose..."
        safe_apt_install python3-pip python3-yaml 2>/dev/null || true
        if pip3 install --break-system-packages podman-compose 2>/dev/null \
            || pip3 install podman-compose 2>/dev/null; then
            success "podman-compose 已通过 pip 安装。"
        else
            warn "podman-compose 未安装，Compose 编排需手动补齐。"
        fi
    fi

    for opt_pkg in fuse-overlayfs passt; do
        if apt-cache show "$opt_pkg" >/dev/null 2>&1; then
            safe_apt_install "$opt_pkg" || warn "${opt_pkg} 安装失败，已跳过。"
        fi
    done
    return 0
}

# 初始化 apps 用户 systemd 会话服务
_podman_enable_user_services() {
    local uid runtime reload_rc auto_rc gc_rc
    uid=$(_podman_get_apps_uid)
    runtime="/run/user/${uid}"
    _podman_bootstrap_apps_session || {
        warn "apps 用户 systemd 会话引导失败，用户级服务可能无法启动。"
        return 1
    }
    # #region agent log
    _podman_debug_log "H2" "podman.sh:_podman_enable_user_services" "pre_enable" \
        "{\"uid\":${uid},\"runtime_exists\":$([ -d \"$runtime\" ] && echo true || echo false),\"linger\":\"$(loginctl show-user ${PODMAN_APPS_USER} -p Linger --value 2>/dev/null || echo unknown)\"}"
    # #endregion
    _podman_apps_exec systemctl --user daemon-reload 2>/dev/null
    reload_rc=$?
    if [[ "$reload_rc" -ne 0 ]]; then
        # #region agent log
        _podman_debug_log "H2" "podman.sh:_podman_enable_user_services" "user_systemd_unavailable" \
            "{\"reload_rc\":${reload_rc}}"
        # #endregion
        warn "systemctl --user 当前不可用（需完整 systemd 登录会话）。定时器单元已部署，登录后可执行 appctl enable --now podman-gc.timer。"
        save_project_config "PODMAN_GC_ENABLED" "false"
        return 0
    fi
    _podman_apps_exec systemctl --user enable --now podman-auto-update.timer 2>/dev/null
    auto_rc=$?
    _podman_apps_exec systemctl --user enable --now podman-gc.timer 2>/dev/null
    gc_rc=$?
    # #region agent log
    _podman_debug_log "H2" "podman.sh:_podman_enable_user_services" "post_enable" \
        "{\"reload_rc\":${reload_rc},\"auto_update_rc\":${auto_rc},\"gc_timer_rc\":${gc_rc}}"
    # #endregion
    save_project_config "PODMAN_GC_ENABLED" "true"
}

# ----------------- 状态检测 -----------------

get_podman_gc_status() {
    if ! id "${PODMAN_APPS_USER}" >/dev/null 2>&1; then
        echo -e "${DIM}○ 未部署${NC}"
        return
    fi
    if _podman_apps_exec systemctl --user is-enabled podman-gc.timer >/dev/null 2>&1; then
        echo -e "${GREEN}●${NC} ${DIM}已开启${NC}"
    else
        echo -e "${YELLOW}●${NC} ${DIM}已关闭${NC}"
    fi
}

# ----------------- 安装 / 卸载 -----------------

install_podman() {
    info "正在部署 Podman Rootless 容器环境..."
    # #region agent log
    _podman_debug_log "H0" "podman.sh:install_podman" "start" \
        "{\"is_cn\":\"${IS_CN_REGION:-false}\",\"maintainer\":\"$(_podman_maintainer_user)\",\"debian\":\"$(. /etc/os-release 2>/dev/null; echo ${VERSION_ID:-unknown})\"}"
    # #endregion

    _podman_remove_docker || return 1

    if ! _podman_install_packages; then
        # #region agent log
        _podman_debug_log "H4" "podman.sh:install_podman" "apt_core_failed" "{}"
        # #endregion
        return 1
    fi

    _podman_create_apps_user || return 1
    _podman_configure_apps || return 1
    _podman_deploy_shell_helpers || return 1
    _podman_enable_user_services || return 1

    if command -v podman >/dev/null 2>&1; then
        # #region agent log
        local _apps_podman_rc=1 _gc_enabled="false"
        runuser -u apps -- podman info --format '{{.Host.Security.Rootless}}' >/dev/null 2>&1 && _apps_podman_rc=0 || true
        _podman_apps_exec systemctl --user is-enabled podman-gc.timer >/dev/null 2>&1 && _gc_enabled="true"
        _podman_debug_log "H5" "podman.sh:install_podman" "install_complete" \
            "{\"apps_podman_rc\":${_apps_podman_rc},\"gc_enabled\":${_gc_enabled}}"
        # #endregion
        success "Podman 环境部署成功！"
        podman --version
        command -v podman-compose >/dev/null 2>&1 && podman-compose version 2>/dev/null || true
        info "运维命令: apppod（容器） / appctl（服务） / applog（日志） / appshell（调试 shell）"
        warn "提醒：部分容器网络可能需要开启系统 IP 转发（TUI 选项 2）。"
        warn "数据目录: ${PODMAN_DATA_DIR}  |  Quadlet 示例: hello.container.example / tls-ferron-certs.container.example（apps 用户下）"
    else
        err "Podman 安装失败，请检查网络或系统资源。"
        return 1
    fi
}

uninstall_podman() {
    info "准备卸载 Podman 环境..."

    local delete_data="N" delete_user="N"
    if [[ "${CI:-}" == "true" ]]; then
        delete_data="N"
        delete_user="N"
    else
        echo -e "${RED}警告：即将删除 Podman 核心程序及 apps 用户配置。${NC}"
        read -p "是否同步清除业务数据 (${PODMAN_DATA_DIR})？ [y/N]: " delete_data
        read -p "是否删除 ${PODMAN_APPS_USER} 用户？ [y/N]: " delete_user
    fi

    if id "${PODMAN_APPS_USER}" >/dev/null 2>&1; then
        _podman_apps_exec systemctl --user stop podman-gc.timer podman-auto-update.timer 2>/dev/null || true
        _podman_apps_exec systemctl --user disable podman-gc.timer podman-auto-update.timer 2>/dev/null || true
        loginctl disable-linger "${PODMAN_APPS_USER}" 2>/dev/null || true
    fi

    _podman_remove_shell_helpers

    local maintainer
    maintainer=$(_podman_maintainer_user)
    if [[ -n "$maintainer" && "$maintainer" != "root" ]]; then
        gpasswd -d "$maintainer" "${PODMAN_APPS_USER}" 2>/dev/null || true
    fi

    rm -f /etc/sudoers.d/debopti-podman
    rm -f /etc/sysctl.d/99-debopti-podman.conf
    sysctl --system >/dev/null 2>&1 || true

    apt-get purge -yq podman podman-docker podman-compose buildah skopeo \
        uidmap fuse-overlayfs passt 2>/dev/null || true
    apt-get autoremove -yq >/dev/null 2>&1 || true

    if [[ "$delete_data" =~ ^[Yy]$ ]]; then
        info "正在清除数据目录..."
        rm -rf "${PODMAN_DATA_DIR}"
    fi

    if [[ "$delete_user" =~ ^[Yy]$ ]]; then
        userdel -r "${PODMAN_APPS_USER}" 2>/dev/null || userdel "${PODMAN_APPS_USER}" 2>/dev/null || true
        sed -i "/^${PODMAN_APPS_USER}:/d" /etc/subuid /etc/subgid 2>/dev/null || true
    fi

    save_project_config "PODMAN_GC_ENABLED" "false"
    success "Podman 已卸载。"
}

# ----------------- 镜像清理定时器管理 -----------------

toggle_podman_gc() {
    if ! id "${PODMAN_APPS_USER}" >/dev/null 2>&1; then
        err "请先安装 Podman 环境。"
        return 1
    fi

    if _podman_apps_exec systemctl --user is-enabled podman-gc.timer >/dev/null 2>&1; then
        info "正在关闭 Podman 镜像清理定时器..."
        _podman_apps_exec systemctl --user disable --now podman-gc.timer || return 1
        save_project_config "PODMAN_GC_ENABLED" "false"
        success "镜像清理定时器已关闭。"
    else
        info "正在开启 Podman 镜像清理定时器（每周执行）..."
        _podman_apps_exec systemctl --user enable --now podman-gc.timer || return 1
        save_project_config "PODMAN_GC_ENABLED" "true"
        success "镜像清理定时器已开启。"
    fi
}
