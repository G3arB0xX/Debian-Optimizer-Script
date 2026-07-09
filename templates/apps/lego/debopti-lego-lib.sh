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

# 以 root 或 sudo 执行 Ferron 推送钩子
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
