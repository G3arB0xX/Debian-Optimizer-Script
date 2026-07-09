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

# 去除首尾空白
_lego_trim_string() {
    local s="${1:-}"
    s="${s#"${s%%[![:space:]]*}"}"
    s="${s%"${s##*[![:space:]]}"}"
    printf '%s' "$s"
}

# 从 DEBOPTI_DOMAINS 提取主域名（首个合法非通配符项）
_lego_primary_from_domains() {
    local domains="${1:-}"
    local d
    IFS=',' read -ra ADDR <<< "$domains"
    for d in "${ADDR[@]}"; do
        d="$(_lego_trim_string "$d")"
        [[ -z "$d" ]] && continue
        if _lego_is_safe_primary_domain "$d"; then
            printf '%s' "$d"
            return 0
        fi
    done
    return 1
}

# 证书文件必须位于 /var/lib/lego/certificates/ 下（防路径注入）
_lego_is_safe_cert_storage_path() {
    local p="${1:-}"
    local resolved=""
    [[ -n "$p" && -e "$p" ]] || return 1
    resolved=$(realpath -s "$p" 2>/dev/null) || return 1
    case "$resolved" in
        /var/lib/lego/certificates/*)
            return 0
            ;;
    esac
    return 1
}

# 校验 env 配置完整性（会设置 domain_args / primary_domain）
_lego_validate_env_config() {
    if [[ -z "${DEBOPTI_DOMAINS:-}" || -z "${DEBOPTI_EMAIL:-}" || -z "${DEBOPTI_PROVIDER:-}" ]]; then
        echo "❌ [Lego] 环境配置不完整（需 DEBOPTI_DOMAINS / DEBOPTI_EMAIL / DEBOPTI_PROVIDER）。" >&2
        return 1
    fi
    if ! _lego_build_domain_args "$DEBOPTI_DOMAINS"; then
        echo "❌ [Lego] 域名列表格式无效: ${DEBOPTI_DOMAINS}" >&2
        return 1
    fi
    case "${DEBOPTI_PROVIDER}" in
        cloudflare)
            if [[ -z "${CLOUDFLARE_DNS_API_TOKEN:-}" ]]; then
                echo "❌ [Lego] Cloudflare 提供商缺少 CLOUDFLARE_DNS_API_TOKEN。" >&2
                return 1
            fi
            ;;
    esac
    return 0
}

# 初始化 lego 工作目录与 env 权限（仅 root 执行）
_lego_prepare_runtime_dirs() {
    [[ $EUID -eq 0 ]] || return 0
    mkdir -p /var/lib/lego/accounts /var/lib/lego/certificates /etc/lego/envs
    chmod 700 /var/lib/lego /var/lib/lego/accounts /var/lib/lego/certificates 2>/dev/null || true
    chmod 700 /etc/lego/envs 2>/dev/null || true
    if [[ -n "${ENV_FILE:-}" && -f "$ENV_FILE" ]]; then
        chmod 600 "$ENV_FILE" 2>/dev/null || true
    fi
}

# deploy-hook：根据 LEGO_HOOK_* 或参数定位 env 文件，成功时设置 ENV_FILE
_lego_resolve_hook_env_file() {
    local candidate="${1:-}"
    candidate="$(_lego_trim_string "$candidate")"

    if _lego_is_safe_primary_domain "$candidate"; then
        if _lego_assert_env_file "/etc/lego/envs/${candidate}.env"; then
            return 0
        fi
    fi

    local d base
    if [[ -n "${LEGO_HOOK_CERT_DOMAINS:-}" ]]; then
        IFS=',' read -ra ADDR <<< "$LEGO_HOOK_CERT_DOMAINS"
        for d in "${ADDR[@]}"; do
            d="$(_lego_trim_string "$d")"
            [[ -z "$d" ]] && continue
            if _lego_is_safe_primary_domain "$d" && _lego_assert_env_file "/etc/lego/envs/${d}.env"; then
                return 0
            fi
            if [[ "$d" == \*.* ]]; then
                base="${d#\*}"
                base="${base#.}"
                if _lego_is_safe_primary_domain "$base" && _lego_assert_env_file "/etc/lego/envs/${base}.env"; then
                    return 0
                fi
            fi
        done
    fi
    return 1
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
        d="$(_lego_trim_string "$d")"
        [[ -z "$d" ]] && continue
        _lego_is_safe_domain_entry "$d" || return 1
        domain_args="$domain_args --domains=$d"
        [[ -n "$primary_domain" ]] || primary_domain="$d"
    done

    [[ -n "$domain_args" && -n "$primary_domain" ]] || return 1
    _lego_is_safe_primary_domain "$primary_domain" || return 1
    return 0
}

# 根据 DEBOPTI_DNS_SKIP_PROPAGATION / 临时强制标记设置 DNS 传播参数
_lego_refresh_dns_propagation_args() {
    lego_dns_propagation_args=()
    if [[ "${DEBOPTI_DNS_SKIP_PROPAGATION:-}" == "true" || "${_LEGO_FORCE_SKIP_PROPAGATION:-}" == "true" ]]; then
        # 跳过全部传播轮询（Lego 使用 wait 替代主动检测）
        lego_dns_propagation_args=(--dns.propagation.wait=0s)
    else
        lego_dns_propagation_args=(
            --dns.resolvers=1.1.1.1:53
            --dns.propagation.disable-rns
        )
    fi
}

_lego_is_propagation_failure_log() {
    local log_file="${1:-}"
    [[ -f "$log_file" ]] || return 1
    grep -qiE 'dns01|propagation|time limit exceeded|expected TXT record|recursive nameservers' "$log_file"
}

# 组装 lego run 的 extra 参数（需已 source env 并调用 _lego_refresh_dns_propagation_args）
_lego_build_extra_args() {
    local mode="${1:-issue}"
    lego_extra_args=(--accept-tos "${lego_dns_propagation_args[@]}")
    if [[ "$mode" == "renew" ]]; then
        lego_extra_args+=(--renew-days 30)
    fi
    if _lego_should_push_ferron; then
        lego_extra_args+=(--deploy-hook "/usr/local/bin/debopti-lego-hook.sh")
    fi
}

# 执行 lego run（root 或 sudo）
_lego_exec_run() {
    if [[ ! -x /usr/local/bin/lego ]]; then
        echo "❌ [Lego] 未找到 /usr/local/bin/lego，请先安装 Lego。" >&2
        return 127
    fi
    # shellcheck disable=SC2086
    if [[ $EUID -eq 0 ]]; then
        /usr/local/bin/lego run \
            --env-file="$ENV_FILE" \
            --email="$DEBOPTI_EMAIL" \
            --dns="$DEBOPTI_PROVIDER" \
            $domain_args \
            --path="/var/lib/lego" \
            "${lego_extra_args[@]}"
    elif command -v sudo >/dev/null 2>&1; then
        sudo /usr/local/bin/lego run \
            --env-file="$ENV_FILE" \
            --email="$DEBOPTI_EMAIL" \
            --dns="$DEBOPTI_PROVIDER" \
            $domain_args \
            --path="/var/lib/lego" \
            "${lego_extra_args[@]}"
    else
        echo "❌ [Lego] 需要 root 权限写入 /var/lib/lego，请以 root 或使用 sudo 运行 debopti。" >&2
        return 1
    fi
}

# 自动续期单次执行（systemd / debopti-lego-renew.sh）
_lego_run_renew_once() {
    _lego_refresh_dns_propagation_args
    _lego_build_extra_args renew
    _lego_exec_run
}

# 手动申请：支持运行中按 s 跳过、失败后 y/N 重试（DEBOPTI_INTERACTIVE_LEG=1 且 TTY）
_lego_run_certificate_flow() {
    local mode="${1:-issue}"
    local log_file
    log_file=$(mktemp)
    trap 'rm -f "${log_file:-}"' EXIT RETURN

    local interactive=false
    if [[ "${DEBOPTI_INTERACTIVE_LEG:-}" == "1" && ( -t 0 || -t 1 ) && -e /dev/tty ]]; then
        interactive=true
    fi

    _LEGO_FORCE_SKIP_PROPAGATION=false
    local attempt=0
    local max_attempts=4

    while [[ $attempt -lt $max_attempts ]]; do
        attempt=$((attempt + 1))
        _lego_refresh_dns_propagation_args
        _lego_build_extra_args "$mode"

        : >"$log_file"
        local rc=0
        local skip_requested=false

        if [[ "$interactive" == "true" && "${DEBOPTI_DNS_SKIP_PROPAGATION:-}" != "true" && "${_LEGO_FORCE_SKIP_PROPAGATION:-}" != "true" ]]; then
            echo "ℹ️ [Lego] DNS 传播校验中… 按 s 可跳过传播校验"
            _lego_exec_run >"$log_file" 2>&1 &
            local lego_pid=$!
            while kill -0 "$lego_pid" 2>/dev/null; do
                local key=""
                if read -r -t 1 -n 1 -s key </dev/tty 2>/dev/null; then
                    if [[ "$key" == "s" || "$key" == "S" ]]; then
                        skip_requested=true
                        kill "$lego_pid" 2>/dev/null || true
                        wait "$lego_pid" 2>/dev/null || true
                        break
                    fi
                fi
            done
            if [[ "$skip_requested" != "true" ]]; then
                wait "$lego_pid" || rc=$?
            else
                rc=1
            fi
        else
            _lego_exec_run >"$log_file" 2>&1 || rc=$?
        fi

        cat "$log_file"

        if [[ $rc -eq 0 ]]; then
            trap - EXIT RETURN
            rm -f "$log_file"
            return 0
        fi

        if [[ "$skip_requested" == "true" ]]; then
            echo "⚠️ [Lego] 已请求跳过传播校验，正在重试..."
            _LEGO_FORCE_SKIP_PROPAGATION=true
            continue
        fi

        if [[ "$interactive" == "true" && "${_LEGO_FORCE_SKIP_PROPAGATION:-}" != "true" ]] \
            && _lego_is_propagation_failure_log "$log_file"; then
            local ans=""
            read -r -p "传播校验失败，是否跳过传播校验并重试？[y/N]: " ans </dev/tty 2>/dev/null || ans=""
            if [[ "$ans" == "y" || "$ans" == "Y" ]]; then
                _LEGO_FORCE_SKIP_PROPAGATION=true
                continue
            fi
        fi

        trap - EXIT RETURN
        rm -f "$log_file"
        return 1
    done

    echo "❌ [Lego] 证书操作重试次数已达上限。" >&2
    trap - EXIT RETURN
    rm -f "$log_file"
    return 1
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

    if ! _lego_is_safe_cert_storage_path "$crt_src" || ! _lego_is_safe_cert_storage_path "$key_src"; then
        echo "❌ [Lego] 证书路径无效或不在 /var/lib/lego/certificates/ 下。" >&2
        return 1
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
    domain="$(_lego_trim_string "$domain")"
    if ! _lego_is_safe_primary_domain "$domain"; then
        echo "❌ [Lego Hook] 无效主域名。" >&2
        return 1
    fi
    if [[ $EUID -eq 0 ]]; then
        /usr/local/bin/debopti-lego-hook.sh "$domain"
    elif command -v sudo >/dev/null 2>&1; then
        sudo /usr/local/bin/debopti-lego-hook.sh "$domain"
    else
        echo "❌ [Lego Hook] 需要 root 权限推送证书至 Ferron。" >&2
        return 1
    fi
}
