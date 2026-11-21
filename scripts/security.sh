#!/bin/bash
# =========================================================
# 安全加固模块 (基于 nftables 目录级管理)
# =========================================================

# ----------------- SSH 安全审计 -----------------
# ----------------- SSH 安全审计与深度加固 -----------------
# 按照 VIBE 指令，实现：非 root 用户创建、Key 登录强制化、高位端口随机化
check_ssh_security() {
    # 交互式提示是否跳过加固（默认执行 [Y/n]，适配容器与轻量化环境）
    local run_ssh_harden="n"
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：自动执行 SSH 安全加固。"
        run_ssh_harden="y"
    else
        local choice="y"
        # 针对 set -e 进行防御性包装，防止意外的输入流中断或 EOF 导致脚本崩溃
        if read -p "是否进行 SSH 安全加固与普通用户创建？(创建 sudo 用户、锁定 root、加固 SSH) [Y/n]: " temp_choice; then
            choice="${temp_choice:-y}"
        fi
        
        if [[ "$choice" =~ ^[Yy]$ ]]; then
            run_ssh_harden="y"
        fi
    fi

    if [[ "$run_ssh_harden" != "y" ]]; then
        info "已跳过 SSH 安全加固与普通用户创建。"
        return 0
    fi

    info "正在执行系统级 SSH 零信任安全加固..."

    # 1. 检测/创建非 root 用户
    local target_user
    target_user=$(detect_or_create_user)

    # 2. 配置 SSH 密钥登录
    setup_user_ssh_key "$target_user"

    # 3. 随机高位端口与防火墙规则
    local old_port
    old_port=$(sshd -T 2>/dev/null | awk '/^port / {print $2}' | head -n 1 || echo 22)
    local new_port=$((RANDOM % 25535 + 40000))
    
    # 记录原始配置用于回退
    local socket_override="/etc/systemd/system/ssh.socket.d/override.conf"
    local has_socket=false
    if systemctl is-active --quiet ssh.socket; then
        has_socket=true
    fi

    apply_ssh_port "$new_port" "$has_socket"
    update_ssh_firewall "$new_port"

    # 4. 验证连通性
    echo -e "\n${YELLOW}⚠️  关键步骤：请勿关闭当前终端！${NC}"
    echo -e "SSH 已切换至端口: ${GREEN}$new_port${NC}"
    echo -e "请在您的本地机器开启一个【新窗口】，尝试使用以下命令登录："
    echo -e "${CYAN}ssh -p $new_port $target_user@$(curl -s4 ifconfig.me || echo "您的服务器IP")${NC}\n"

    local confirmed=false
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        info "CI/非交互模式：跳过端口连接验证，直接应用配置。"
        confirmed=true
    else
        read -p "您是否已成功通过新端口登录？(y/n): " confirm
        if [[ "$confirm" == "y" || "$confirm" == "Y" ]]; then
            confirmed=true
        else
            warn "验证未通过，正在回滚配置..."
            rollback_ssh_changes "$old_port" "$has_socket" "$new_port"
            return 1
        fi
    fi

    # 5. 终极加固：禁止 Root、禁止密码、锁定 Root
    if [[ "$confirmed" == "true" ]]; then
        lockdown_ssh_system
        success "SSH 安全加固任务圆满完成。"
    fi
}

detect_or_create_user() {
    # 查找 UID >= 1000 的普通用户 (排除 nobody)
    local users=$(awk -F: '$3 >= 1000 && $1 != "nobody" {print $1}' /etc/passwd)
    
    if [[ -n "$users" ]]; then
        info "检测到现有普通用户: $(echo $users | tr '\n' ' ')"
        local selected_user=$(echo $users | awk '{print $1}')
        
        if [[ -n "${CI:-}" || ! -t 0 ]]; then
            info "CI/非交互模式：自动选择用户 '$selected_user'。"
            echo "$selected_user"
            return
        fi

        read -p "是否使用现有用户 '$selected_user' 进行加固？(y/n): " use_existing
        if [[ "$use_existing" == "y" || "$use_existing" == "Y" ]]; then
            echo "$selected_user"
            return
        fi
    fi

    # 创建新用户
    local username
    local attempt=0
    while [[ $attempt -lt 3 ]]; do
        attempt=$((attempt + 1))
        if [[ -n "${CI:-}" || ! -t 0 ]]; then
            # 非交互模式下的默认行为
            username="admin"
            [[ -n "${TEST_USERNAME:-}" ]] && username="${TEST_USERNAME}"
        else
            read -p "请输入要创建的新用户名 (默认: admin): " username
            username=${username:-admin}
        fi

        if [[ "$username" =~ ^[a-z][-a-z0-9]*$ ]]; then
            if id "$username" &>/dev/null; then
                if [[ -n "${CI:-}" || ! -t 0 ]]; then
                    # 如果用户已存在且为非交互模式，直接跳过创建
                    info "用户 $username 已存在，直接使用。"
                    echo "$username"
                    return
                fi
                err "用户已存在，请换一个名字。"
            else
                break
            fi
        else
            err "用户名格式不合法 (仅支持小写字母和数字，以字母开头)。"
            [[ -n "${CI:-}" || ! -t 0 ]] && die "自动化创建用户失败: 用户名非法"
        fi
    done
    [[ $attempt -ge 3 ]] && die "多次输入错误，加固任务终止。"

    # 交互式或自动化设置密码
    useradd -m -s /bin/bash "$username"
    if [[ -n "${CI:-}" || ! -t 0 ]]; then
        local pass="${TEST_USER_PASSWORD:-admin123}"
        echo "$username:$pass" | chpasswd
        info "CI/非交互模式：已为用户 $username 设置随机/预设密码。"
    else
        info "请为用户 $username 设置密码 (输入时不可见):"
        passwd "$username"
    fi

    # 赋予 sudo 权限
    if grep -q "^sudo:" /etc/group; then
        usermod -aG sudo "$username"
    elif grep -q "^wheel:" /etc/group; then
        usermod -aG wheel "$username"
    fi
    
    info "用户 $username 创建成功并已加入 sudo 组。"
    echo "$username"
}

setup_user_ssh_key() {
    local user=$1
    local user_home=$(eval echo ~$user)
    local ssh_dir="$user_home/.ssh"

    mkdir -p "$ssh_dir"
    chmod 700 "$ssh_dir"

    printf "\n${YELLOW}配置 SSH 密钥登录 (Ed25519)${NC}\n"
    printf "请在您的本地电脑运行: ${CYAN}ssh-keygen -t ed25519${NC}\n"
    printf "然后将生成的公钥 (.pub 文件内容) 粘贴到下方：\n"
    
    local pub_key=""
    if [[ -n "${TEST_SSH_PUBKEY:-}" ]]; then
        pub_key="${TEST_SSH_PUBKEY}"
        info "从环境变量加载 SSH 公钥。"
    elif [[ -n "${CI:-}" || ! -t 0 ]]; then
        # 生成临时的 Ed25519 密钥对以便完成流程 (仅用于测试/CI)
        info "CI/非交互模式：生成测试用临时密钥对..."
        ssh-keygen -t ed25519 -N "" -f "$INSTALL_DIR/test_key" >/dev/null
        pub_key=$(cat "$INSTALL_DIR/test_key.pub")
    else
        while [[ -z "$pub_key" ]]; do
            read -p "粘贴公钥: " pub_key
            if [[ -n "$pub_key" && ! "$pub_key" =~ ssh-ed25519 ]]; then
                warn "检测到非 Ed25519 格式密钥，为了安全建议使用 ed25519。请确认或重新粘贴。"
                pub_key=""
            fi
        done
    fi

    echo "$pub_key" > "$ssh_dir/authorized_keys"
    chmod 600 "$ssh_dir/authorized_keys"
    chown -R "$user:$user" "$ssh_dir"
    info "密钥已成功注入 $user 账户。"
}

apply_ssh_port() {
    local port=$1
    local has_socket=$2

    if [[ "$has_socket" == "true" ]]; then
        info "检测到系统使用 ssh.socket，正在应用 Systemd Override..."
        render_template "templates/security/ssh.socket.override.conf" "/etc/systemd/system/ssh.socket.d/override.conf" "PORT=$port"
        systemctl daemon-reload
        systemctl restart ssh.socket
    else
        info "修改 /etc/ssh/sshd_config 端口..."
        sed -i "s/^#\?Port [0-9]*/Port $port/" /etc/ssh/sshd_config
        systemctl restart ssh
    fi
}

rollback_ssh_changes() {
    local old_port=$1
    local has_socket=$2
    local new_port=$3

    info "执行配置回滚..."
    if [[ "$has_socket" == "true" ]]; then
        rm -f /etc/systemd/system/ssh.socket.d/override.conf
        systemctl daemon-reload
        systemctl restart ssh.socket
    else
        sed -i "s/^Port $new_port/Port $old_port/" /etc/ssh/sshd_config
        systemctl restart ssh
    fi

    # 恢复防火墙规则至旧端口，确保回滚后连通性
    update_ssh_firewall "$old_port"
    # 同时清理可能存在的旧命名规则文件
    rm -f "${NFT_CONF_DIR}/SSH_Hardened_Port.nft" "${NFT_CONF_DIR}/SSH_Listen_Port.nft" "${NFT_CONF_DIR}/SSH_Access_Port.nft"
    if [[ "${DEBOPTY_F2B_NO_SYS:-0}" != "1" ]]; then
        nft -f "${NFT_MAIN_CONF:-/etc/nftables.conf}" 2>/dev/null || true
    fi
    sync_fail2ban_public_ports --force-reload
    
    warn "配置已回滚至端口 $old_port。请检查网络环境后重试。"
}

lockdown_ssh_system() {
    info "执行最终安全加固 (禁止密码/Root登录)..."

    # 修改 sshd_config
    local config="/etc/ssh/sshd_config"
    set_conf_value "$config" "PermitRootLogin" "no" " "
    set_conf_value "$config" "PasswordAuthentication" "no" " "
    set_conf_value "$config" "PubkeyAuthentication" "yes" " "
    
    # 针对 Debian 12 的额外加固：确保 ssh.service 也重启以应用配置
    systemctl restart ssh

    # 锁定 root 密码
    info "锁定 Root 账户密码..."
    passwd -l root
    
    # 持久化标记
    save_project_config "SSH_HARDENED" "true"
}

# ----------------- 现代防火墙接口 (nftables) -----------------
# 采用目录级管理，确保规则的原子化与可撤销性
NFT_BASE_D="${NFT_BASE_D:-/etc/nftables.d}"
NFT_CONF_DIR="${NFT_CONF_DIR:-${NFT_BASE_D}/debopti}"
NFT_MAIN_CONF="${NFT_MAIN_CONF:-/etc/nftables.conf}"

# [内部] 迁移旧版 nftables 路径
migrate_nft_paths() {
    local old_dir="/etc/nftables/debopti.d"
    if [[ -d "$old_dir" ]]; then
        info "检测到旧版防火墙目录，正在执行结构迁移..."
        mkdir -p "$NFT_CONF_DIR"
        # 搬迁现有 .nft 规则
        cp -rn "$old_dir"/*.nft "$NFT_CONF_DIR/" 2>/dev/null || true
        # 清理旧的 include 指令并删除旧目录
        [[ -f "$NFT_MAIN_CONF" ]] && sed -i "\|include \"$old_dir/\*.nft\"|d" "$NFT_MAIN_CONF"
        rm -rf "/etc/nftables"
        # 统一规则命名：清理旧版可能残留的冗余规则
        rm -f "${NFT_CONF_DIR}/SSH_Hardened_Port.nft" "${NFT_CONF_DIR}/SSH_Listen_Port.nft"
        info "迁移完成。"
    fi
}

# [内部] 更新主配置文件中的 SSH 端口规则
update_ssh_firewall() {
    local port=$1
    local conf="${NFT_MAIN_CONF:-/etc/nftables.conf}"

    # 如果主配置尚未初始化，则先执行初始化逻辑
    if [[ ! -f "$conf" ]]; then
        setup_security
        return
    fi

    info "同步更新防火墙主配置中的管理端口: $port..."
    if grep -q "comment \"SSH_Access_Port\"" "$conf"; then
        sed -i "s/tcp dport [0-9, \-]* accept comment \"SSH_Access_Port\"/tcp dport $port accept comment \"SSH_Access_Port\"/" "$conf"
    else
        # 兜底逻辑：如果主配置缺少该行，则尝试插入到 input 链末尾（include 之前）
        sed -i "/chain input {/a \        tcp dport $port accept comment \"SSH_Access_Port\"" "$conf"
    fi

    # 尝试加载规则，若失败则回退（虽然 sed 很难出错，但这是防御性编程）
    if [[ "${DEBOPTY_F2B_NO_SYS:-0}" != "1" ]]; then
        if ! nft -f "$conf" 2>/dev/null; then
            warn "防火墙主配置加载失败，请检查规则语法。"
            return 1
        fi
    fi

    sync_fail2ban_public_ports --force-reload
}

add_fw_rule() {
    local port=$1
    local proto=$2
    local comment=$3
    local rule_file="${NFT_CONF_DIR}/${comment// /_}.nft"
    local conf="${NFT_MAIN_CONF:-/etc/nftables.conf}"

    info "下发 nftables 规则: $port/$proto ($comment)..."

    # 确保管理目录存在
    if [[ ! -d "$NFT_CONF_DIR" ]]; then
        if [[ "${DEBOPTY_F2B_NO_SYS:-0}" == "1" ]]; then
            mkdir -p "$NFT_CONF_DIR"
        else
            setup_security
        fi
    fi

    # 处理端口格式：将 - 或 : 统一为 " - " 以符合 nft 范围语法
    local nft_port="${port//-/ - }"
    nft_port="${nft_port//:/ - }"
    # 如果包含空格(范围)或逗号(列表)，且未被花括号包围，则包围之
    if [[ ( "$nft_port" == *" "* || "$nft_port" == *","* ) && "$nft_port" != "{"* ]]; then
        nft_port="{ $nft_port }"
    fi

    # 处理协议并构造规则集 (支持 tcp/udp 复合协议)
    local rules=""
    IFS='/' read -ra ADDR <<< "$proto"
    for p in "${ADDR[@]}"; do
        rules="${rules}        $p dport $nft_port accept comment \"$comment\"\n"
    done

    # 构造原子规则文件
    # 使用 printf %b 处理换行符
    render_template "templates/security/rule_template.nft" "$rule_file" "RULES=$(printf "%b" "$rules")"

    # 执行原子加载，防止语法错误导致防火墙整体崩溃
    if [[ "${DEBOPTY_F2B_NO_SYS:-0}" != "1" ]]; then
        if ! nft -f "$rule_file" 2>/dev/null; then
            warn "规则语法校验失败 ($port/$proto)，尝试回退..."
            rm -f "$rule_file"
            return 1
        fi
    fi
    
    # 确保主配置加载 .d 根目录
    if [[ -f "$conf" ]] && ! grep -q "include \"$NFT_BASE_D/\*.nft\"" "$conf"; then
        echo "include \"$NFT_BASE_D/*.nft\"" >> "$conf"
    fi

    # 确保模块入口文件存在 (50-debopti.nft -> debopti/*.nft)
    local mod_entry="${NFT_BASE_D}/50-debopti.nft"
    if [[ ! -f "$mod_entry" ]]; then
        echo "include \"$NFT_CONF_DIR/*.nft\"" > "$mod_entry"
    fi

    sync_fail2ban_public_ports --force-reload
    success "规则已持久化。"
}

remove_fw_rule() {
    local comment=$1
    local rule_file="${NFT_CONF_DIR}/${comment// /_}.nft"
    local conf="${NFT_MAIN_CONF:-/etc/nftables.conf}"

    if [[ -f "$rule_file" ]]; then
        info "移除 nftables 规则: ${comment}..."
        rm -f "$rule_file"
        if [[ "${DEBOPTY_F2B_NO_SYS:-0}" != "1" && -f "$conf" ]]; then
            nft -f "$conf" >/dev/null 2>&1 || warn "防火墙主配置重载失败，请检查规则语法。"
        fi
        sync_fail2ban_public_ports --force-reload
    fi
}

# ----------------- Fail2ban 最佳实践与全公网端口自动同步 -----------------

detect_fail2ban_version_ge_011() {
    local ver_str=""
    if command -v fail2ban-client >/dev/null 2>&1; then
        ver_str="$(fail2ban-client --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+' | head -n 1 || true)"
    fi
    if [[ -z "$ver_str" ]] && command -v dpkg-query >/dev/null 2>&1; then
        ver_str="$(dpkg-query -W -f='${Version}\n' fail2ban 2>/dev/null | grep -oE '[0-9]+\.[0-9]+' | head -n 1 || true)"
    fi
    [[ -z "$ver_str" ]] && return 0

    local major="${ver_str%%.*}"
    local minor="${ver_str#*.}"
    if (( major > 0 || (major == 0 && minor >= 11) )); then
        return 0
    fi
    return 1
}

detect_fail2ban_backend() {
    if [[ -d /run/systemd/system ]] && { python3 -c 'import systemd.journal' >/dev/null 2>&1 || dpkg -s python3-systemd >/dev/null 2>&1; }; then
        printf 'systemd\n'
    else
        printf 'auto\n'
    fi
}

cleanup_legacy_fail2ban_jail_local() {
    local legacy_file="${DEBOPTY_F2B_ROOT:-}/etc/fail2ban/jail.local"
    [[ -f "$legacy_file" ]] || return 0

    local line_count
    line_count="$(wc -l < "$legacy_file" | tr -d ' ')"
    if (( line_count <= 15 )) \
        && grep -q '^banaction = nftables-multiport$' "$legacy_file" \
        && grep -q '^banaction_allports = nftables-allports$' "$legacy_file" \
        && grep -q '^logpath = /var/log/auth.log$' "$legacy_file" \
        && grep -q '^bantime = 86400$' "$legacy_file" \
        && grep -q '^findtime = 600$' "$legacy_file"; then
        info "清理旧版自动生成的 /etc/fail2ban/jail.local（已迁移至 jail.d/99-debopty.local）..."
        rm -f "$legacy_file"
    fi
}

sync_fail2ban_public_ports() {
    local sync_bin="${DEBOPTY_F2B_SYNC_BIN:-/usr/local/bin/debopty-f2b-sync}"
    local fallback_sync="${SCRIPT_DIR:-/opt/debopty}/templates/security/debopty-f2b-sync.sh"

    if [[ -x "$sync_bin" ]]; then
        "$sync_bin" "$@" || warn "Fail2ban 公网端口防护同步失败。"
    elif [[ -f "$fallback_sync" ]]; then
        bash "$fallback_sync" "$@" || warn "Fail2ban 公网端口防护同步失败。"
    fi
}

setup_fail2ban() {
    local ssh_port="${1:-}"
    if [[ -z "$ssh_port" ]]; then
        ssh_port="$(sshd -T 2>/dev/null | awk '/^port / {print $2}' | head -n 1 || true)"
        ssh_port="${ssh_port:-22}"
    fi

    info "配置 Fail2ban 最佳实践与全公网开放端口自动防护..."

    safe_apt_install fail2ban || return 1
    safe_apt_install python3-systemd >/dev/null 2>&1 || warn "python3-systemd 不可用，Fail2ban 将回退至日志文件模式。"

    local f2b_backend
    f2b_backend="$(detect_fail2ban_backend)"

    local f2b_default_bantime="24h"
    local f2b_sshd_bantime="24h"
    local f2b_increment_block="# Incremental bantime requires Fail2ban >= 0.11"
    if detect_fail2ban_version_ge_011; then
        f2b_default_bantime="1h"
        f2b_sshd_bantime="1h"
        f2b_increment_block=$'bantime.increment = true\nbantime.factor = 2\nbantime.maxtime = 7d\nbantime.rndtime = 5m'
    fi

    local sshd_log_directive="logpath = /var/log/auth.log"
    if [[ "$f2b_backend" == "systemd" ]]; then
        sshd_log_directive="backend = systemd"
    fi

    # 1. 部署核心最佳实践 Jail 配置与公网端口过滤器
    render_template "templates/security/fail2ban_jail.local" "/etc/fail2ban/jail.d/99-debopty.local" \
        "F2B_BACKEND=${f2b_backend}" \
        "F2B_DEFAULT_BANTIME=${f2b_default_bantime}" \
        "F2B_SSHD_BANTIME=${f2b_sshd_bantime}" \
        "F2B_INCREMENT_BLOCK=${f2b_increment_block}" \
        "SSH_PORT=${ssh_port}" \
        "SSHD_LOG_DIRECTIVE=${sshd_log_directive}"

    render_template "templates/security/fail2ban_filter_public_ports.conf" "/etc/fail2ban/filter.d/debopty-public-ports.conf"

    # 仅清理旧版 debopti 自动生成的 12 行 jail.local，不覆盖用户自定义配置
    cleanup_legacy_fail2ban_jail_local

    # 2. 部署独立公网端口同步助手与运行时模板（独立于 INSTALL_DIR）
    local script_dir="${SCRIPT_DIR:-/opt/debopty}"
    install -D -m 0755 "${script_dir}/templates/security/debopty-f2b-sync.sh" "/usr/local/bin/debopty-f2b-sync"
    install -D -m 0644 "${script_dir}/templates/security/fail2ban_ports.local" "/usr/local/share/debopty/templates/security/fail2ban_ports.local"
    install -D -m 0644 "${script_dir}/templates/security/f2b_public_ports_guard.nft" "/usr/local/share/debopty/templates/security/f2b_public_ports_guard.nft"

    # 3. 部署 Systemd 自动监听单元与 nftables 重载联动 Hook
    render_template "templates/security/debopty-f2b-port-sync.service" "/etc/systemd/system/debopty-f2b-port-sync.service"
    render_template "templates/security/debopty-f2b-port-sync.path" "/etc/systemd/system/debopty-f2b-port-sync.path"
    render_template "templates/security/debopty-f2b-port-sync.timer" "/etc/systemd/system/debopty-f2b-port-sync.timer"
    render_template "templates/security/nftables-fail2ban-override.conf" "/etc/systemd/system/nftables.service.d/10-debopty-fail2ban.conf"

    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl enable fail2ban >/dev/null 2>&1 || true
    systemctl restart fail2ban >/dev/null 2>&1 || warn "无法重启 fail2ban。"
    systemctl enable --now debopty-f2b-port-sync.path debopty-f2b-port-sync.timer >/dev/null 2>&1 || true

    # 4. 立即同步所有公网开放端口并确保 Fail2ban nftables 规则表生效
    sync_fail2ban_public_ports --force-reload
}

# 修补遗留系统的 IPv6 NDP 放行规则 (无需重新生成整个配置文件)
patch_nftables_ndp() {
    local conf="${NFT_MAIN_CONF:-/etc/nftables.conf}"
    if [[ -f "$conf" ]] && grep -q "SSH_Access_Port" "$conf" && ! grep -q "nd-router-solicit" "$conf"; then
        info "检测到系统防火墙缺少 IPv6 NDP 放行规则，正在自动修补..."
        # 在 icmpv6 echo-request 放行规则下自动追加 NDP 规则
        sed -i '/icmpv6 type echo-request/a \
        # 核心：放行 IPv6 邻居发现协议 (NDP)，防止 IPv6 断网失联\
        ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } accept' "$conf"
        
        systemctl reload nftables >/dev/null 2>&1 || true
        success "防火墙 IPv6 路由寻址规则已修补完毕。"
    fi
}

setup_security() {
    info "初始化系统级安全防御引擎 (nftables + Fail2ban)..."

    # 1. 彻底移除 UFW 以破除规则冲突死锁
    if command -v ufw >/dev/null 2>&1; then
        warn "清理旧版 UFW 引擎..."
        ufw disable >/dev/null 2>&1 || true
        apt-get purge -yq ufw >/dev/null 2>&1
    fi

    # 2. 安装并启用基础套件
    safe_apt_install nftables fail2ban || return 1

    # 3. 获取当前 SSH 端口 (用于主规则与 Fail2ban)
    local ssh_port
    ssh_port=$(sshd -T 2>/dev/null | awk '/^port / {print $2}' | head -n 1 || true)
    ssh_port=${ssh_port:-22}

    # 4. 构建规范化 /etc/nftables.conf
    migrate_nft_paths  # 执行结构迁移
    mkdir -p "$NFT_CONF_DIR" "$NFT_BASE_D"
    render_template "templates/security/nftables.conf" "$NFT_MAIN_CONF" "SSH_PORT=$ssh_port"

    # 4. 生成模块入口文件
    echo "include \"$NFT_CONF_DIR/*.nft\"" > "${NFT_BASE_D}/50-debopti.nft"

    # 5. 激活服务
    systemctl enable --now nftables >/dev/null 2>&1 || warn "无法启动 nftables (可能由于缺少内核权限)。"
    nft -f "$NFT_MAIN_CONF" >/dev/null 2>&1 || warn "nftables 规则加载失败 (可能由于缺少内核权限)。"

    # 6. 配置 Fail2ban 最佳实践、全公网开放端口防护与自动同步
    setup_fail2ban "$ssh_port"
    success "全局安全引擎配置指令已完成。"
}
