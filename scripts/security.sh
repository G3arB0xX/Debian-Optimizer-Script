#!/bin/bash
# =========================================================
# 安全加固模块 (基于 nftables 目录级管理)
# =========================================================

# ----------------- SSH 安全审计 -----------------
check_ssh_security() {
    info "正在进行 SSH 零信任安全审计..."
    # 动态解析当前生效的 SSH 配置，而非盲目读取文件
    local ssh_port=$(sshd -T 2>/dev/null | awk '/^port / {print $2}' | head -n 1 || true)
    ssh_port=${ssh_port:-22}
    
    local pass_auth=$(sshd -T 2>/dev/null | awk '/^passwordauthentication / {print $2}' || true)
    
    # 核心红线：禁止在公网暴露 22 端口的同时开启密码登录
    if [[ "$ssh_port" == "22" && "$pass_auth" == "yes" ]]; then
        echo -e "${RED}严重安全风险拦截：检测到服务器暴露 22 端口且允许密码登录。${NC}"
        echo -e "为了保护您的资产，脚本已强制停止。请配置 Key 登录或更换端口后重试。"
        return 1
    fi
    info "SSH 安全审计通过。"
}

# ----------------- 现代防火墙接口 (nftables) -----------------
# 采用目录级管理，确保规则的原子化与可撤销性
NFT_CONF_DIR="/etc/nftables/debopti.d"

add_fw_rule() {
    local port=$1
    local proto=$2
    local comment=$3
    local rule_file="${NFT_CONF_DIR}/${comment// /_}.nft"

    info "下发 nftables 规则: $port/$proto ($comment)..."

    # 确保管理目录存在
    if [[ ! -d "$NFT_CONF_DIR" ]]; then
        setup_security
    fi

    # 构造原子规则文件
    # 使用 inet 族以同时支持 IPv4 和 IPv6
    cat > "$rule_file" << EOF
table inet filter {
    chain input {
        ${proto//\// } dport { ${port//:/ - } } accept comment "$comment"
    }
}
EOF
    # 语法释义：
    # ${proto//\// }：将 tcp/udp 转换为 tcp udp，适配 nft 语法
    # ${port//:/ - }：将 11010:11015 转换为 11010 - 11015，适配 nft 范围语法

    # 执行原子加载，防止语法错误导致防火墙整体崩溃
    if ! nft -f "$rule_file" 2>/dev/null; then
        warn "规则语法校验失败，尝试回退并应用..."
        rm -f "$rule_file"
        return 1
    fi
    
    # 确保主配置文件包含此目录
    if ! grep -q "include \"$NFT_CONF_DIR/\*.nft\"" /etc/nftables.conf; then
        echo "include \"$NFT_CONF_DIR/*.nft\"" >> /etc/nftables.conf
    fi
    info "✅ 规则已持久化。"
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
    apt-get update -yq >/dev/null 2>&1
    apt-get install -yq nftables fail2ban >/dev/null 2>&1

    # 3. 构建规范化 /etc/nftables.conf
    # 采用标准 hook 架构，优先处理 established 连接以最大化性能
    mkdir -p "$NFT_CONF_DIR"
    cat > /etc/nftables.conf << EOF
#!/usr/sbin/nft -f
flush ruleset

table inet filter {
    chain input {
        type filter hook input priority 0; policy drop;

        # 允许环回接口数据流
        iifname "lo" accept

        # 核心：允许已建立和关联的报文 (保证 TCP 握手响应)
        ct state established,related accept

        # 丢弃所有无效状态报文 (防范扫描与畸形包攻击)
        ct state invalid drop

        # 允许 ICMP/ICMPv6 (Ping) 并进行限速，防止 Ping 洪水攻击
        ip protocol icmp icmp type echo-request limit rate 5/second accept
        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept
    }

    chain forward {
        type filter hook forward priority 0; policy accept;
    }

    chain output {
        type filter hook output priority 0; policy accept;
    }
}

# 引入动态规则目录
include "$NFT_CONF_DIR/*.nft"
EOF

    # 4. 获取当前 SSH 端口并生成首条持久化规则
    local ssh_port=$(sshd -T 2>/dev/null | awk '/^port / {print $2}' | head -n 1 || true)
    ssh_port=${ssh_port:-22}
    add_fw_rule "$ssh_port" "tcp" "SSH_Listen_Port"

    # 5. 激活服务
    systemctl enable --now nftables
    nft -f /etc/nftables.conf || die "nftables 引擎启动异常，请检查系统日志。"

    # 6. 配置 Fail2ban 联动 nftables
    # 使用 nftables-multiport 动作直接在内核层阻断黑客 IP
    info "同步配置 Fail2ban 联动策略..."
    cat > /etc/fail2ban/jail.local << EOF
[DEFAULT]
banaction = nftables-multiport
banaction_allports = nftables-allports

[sshd]
enabled = true
port = $ssh_port
filter = sshd
logpath = /var/log/auth.log
maxretry = 3
bantime = 86400
findtime = 600
EOF
    systemctl restart fail2ban
    systemctl enable fail2ban
    info "✅ 全局安全引擎已就绪。"
}
