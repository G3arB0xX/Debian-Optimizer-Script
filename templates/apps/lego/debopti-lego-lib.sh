#!/bin/bash
# =========================================================
# Lego 模块共享函数库 (Debopti 托管，仅 source，不可直接执行)
# =========================================================

# 校验并规范化 env 文件路径，成功时设置 ENV_FILE 变量
_lego_assert_env_file() {
    local candidate="${1:-}"
    local resolved=""

    [[ -n "$candidate" && -f "$candidate" ]] || return 1
    resolved=$(realpath -s "$candidate" 2>/dev/null) || return 1
    case "$resolved" in
        /etc/lego/envs/*.env)
            ENV_FILE="$resolved"
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

# 主域名（用作文件名）不允许通配符与路径字符
_lego_is_safe_primary_domain() {
    local d="${1:-}"
    [[ -n "$d" && "$d" != *"*"* && "$d" != *"/"* && "$d" != *".."* \
        && "$d" != *'"'* && "$d" != *"'"* && "$d" != *" "* \
        && "$d" != *$'\n'* && "$d" != *$'\r'* && "$d" != *";"* ]]
}

# SAN 域名列表项（允许 *.example.com）
_lego_is_safe_domain_entry() {
    local d="${1:-}"
    [[ -n "$d" && "$d" != *"/"* && "$d" != *".."* \
        && "$d" != *'"'* && "$d" != *"'"* && "$d" != *" "* \
        && "$d" != *$'\n'* && "$d" != *$'\r'* && "$d" != *";"* ]]
}

# 解析 DEBOPTI_DOMAINS，输出 domain_args 与 primary_domain
_lego_build_domain_args() {
    local domains="${1:-}"
    domain_args=""
    primary_domain=""

    IFS=',' read -ra ADDR <<< "$domains"
    for d in "${ADDR[@]}"; do
        d="${d#"${d%%[![:space:]]*}"}"
        d="${d%"${d##*[![:space:]]}"}"
        [[ -z "$d" ]] && continue
        _lego_is_safe_domain_entry "$d" || return 1
        domain_args="$domain_args --domains=$d"
    done

    [[ -n "$domain_args" ]] || return 1
    primary_domain="${ADDR[0]}"
    primary_domain="${primary_domain#"${primary_domain%%[![:space:]]*}"}"
    primary_domain="${primary_domain%"${primary_domain##*[![:space:]]}"}"
    _lego_is_safe_primary_domain "$primary_domain" || return 1
    return 0
}

_lego_ferron_installed() {
    [[ -d /etc/ferron ]] || command -v ferron >/dev/null 2>&1
}

# 是否应将证书同步至 Ferron（需显式开启 DEBOPTI_FERRON_PUSH 且 Ferron 已安装）
_lego_should_push_ferron() {
    [[ "${DEBOPTI_FERRON_PUSH:-}" == "true" ]] && _lego_ferron_installed
}

# 将证书复制到 /etc/ferron/certs 并重载 Ferron
_lego_push_certs_to_ferron() {
    local primary_domain="${1:-}"
    local crt_src="${2:-/var/lib/lego/certificates/${primary_domain}.crt}"
    local key_src="${3:-/var/lib/lego/certificates/${primary_domain}.key}"

    if ! _lego_is_safe_primary_domain "$primary_domain"; then
        echo "❌ [Lego] 无效主域名，无法推送至 Ferron。" >&2
        return 1
    fi

    if ! _lego_ferron_installed; then
        echo "⚠️ [Lego] Ferron 未安装，跳过证书同步。" >&2
        return 0
    fi

    if [[ ! -f "$crt_src" || ! -f "$key_src" ]]; then
        echo "❌ [Lego] 找不到证书文件: $crt_src" >&2
        return 1
    fi

    echo "🔹 [Lego] 同步证书至 Ferron: $primary_domain ..."
    mkdir -p /etc/ferron/certs
    install -m 644 -o ferron -g ferron "$crt_src" "/etc/ferron/certs/${primary_domain}.crt" 2>/dev/null \
        || cp "$crt_src" "/etc/ferron/certs/${primary_domain}.crt"
    install -m 600 -o ferron -g ferron "$key_src" "/etc/ferron/certs/${primary_domain}.key" 2>/dev/null \
        || cp "$key_src" "/etc/ferron/certs/${primary_domain}.key"

    chown -R ferron:ferron /etc/ferron/certs 2>/dev/null || true
    chmod 700 /etc/ferron/certs
    chmod 600 "/etc/ferron/certs/${primary_domain}.key"
    chmod 644 "/etc/ferron/certs/${primary_domain}.crt"

    if systemctl is-active --quiet ferron 2>/dev/null; then
        systemctl reload-or-restart ferron 2>/dev/null || systemctl restart ferron
        echo "✨ [Lego] Ferron 已载入新证书并重载。"
    fi
    return 0
}

# 以 root 或 sudo 执行 Ferron 推送（读取 env 配置）
_lego_run_hook() {
    local domain="${1:-}"
    if [[ $EUID -eq 0 ]]; then
        /usr/local/bin/debopti-lego-hook.sh "$domain"
    elif command -v sudo >/dev/null 2>&1; then
        sudo /usr/local/bin/debopti-lego-hook.sh "$domain"
    else
        echo "❌ [Lego Hook] 需要 root 权限推送证书至 Ferron。" >&2
        return 1
    fi
}
