#!/bin/bash
# =========================================================
# Lego 单域名证书申请/续期脚本 (Debopti 托管)
# 用法: debopti-lego-run-once.sh <env_file> [issue|renew|auto]
# =========================================================
set -euo pipefail

if [[ ! -f /usr/local/bin/debopti-lego-lib.sh ]]; then
    echo "❌ [Lego] 缺少 debopti-lego-lib.sh，请重新安装 Lego。" >&2
    exit 1
fi

# shellcheck source=/dev/null
source /usr/local/bin/debopti-lego-lib.sh

ENV_FILE=""
MODE="${2:-auto}"

if ! _lego_assert_env_file "${1:-}"; then
    echo "❌ [Lego] 缺少或无效的环境配置文件（仅允许 /etc/lego/envs/*.env）。" >&2
    exit 1
fi

if [[ "$MODE" != "issue" && "$MODE" != "renew" && "$MODE" != "auto" ]]; then
    echo "❌ [Lego] 无效模式: $MODE（允许: issue, renew, auto）" >&2
    exit 1
fi

if [[ ! -x /usr/local/bin/lego ]]; then
    echo "❌ [Lego] 未找到 /usr/local/bin/lego，请先安装 Lego。" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

if [[ -z "${DEBOPTI_DOMAINS:-}" || -z "${DEBOPTI_EMAIL:-}" || -z "${DEBOPTI_PROVIDER:-}" ]]; then
    echo "❌ [Lego] 环境配置不完整（需 DEBOPTI_DOMAINS / DEBOPTI_EMAIL / DEBOPTI_PROVIDER）。" >&2
    exit 1
fi

if ! _lego_build_domain_args "$DEBOPTI_DOMAINS"; then
    echo "❌ [Lego] 域名列表格式无效: ${DEBOPTI_DOMAINS}" >&2
    exit 1
fi

cert_path="/var/lib/lego/certificates/${primary_domain}.crt"

if [[ "$MODE" == "auto" ]]; then
    if [[ -f "$cert_path" ]]; then
        MODE="renew"
    else
        MODE="issue"
    fi
fi

if [[ "$MODE" == "issue" && -f "$cert_path" ]]; then
    echo "ℹ️ [Lego] 证书已存在，跳过首次申请: $primary_domain"
    exit 0
fi

if [[ "$MODE" == "renew" && ! -f "$cert_path" ]]; then
    echo "⚠️ [Lego] 证书尚未申请，无法续期: $primary_domain" >&2
    exit 1
fi

if [[ $EUID -eq 0 ]]; then
    mkdir -p /var/lib/lego/accounts /var/lib/lego/certificates /etc/lego/envs
    chmod 700 /var/lib/lego /var/lib/lego/accounts /var/lib/lego/certificates 2>/dev/null || true
    chmod 700 /etc/lego/envs 2>/dev/null || true
    chmod 600 "$ENV_FILE" 2>/dev/null || true
fi

extra_args=(--accept-tos)
if [[ "$MODE" == "renew" ]]; then
    extra_args+=(--renew-days 30)
fi
if [[ "${DEBOPTI_FERRON_PUSH:-}" == "true" ]]; then
    extra_args+=(--deploy-hook "/usr/local/bin/debopti-lego-hook.sh")
fi

_run_lego() {
    # shellcheck disable=SC2086
    /usr/local/bin/lego run \
        --env-file="$ENV_FILE" \
        --email="$DEBOPTI_EMAIL" \
        --dns="$DEBOPTI_PROVIDER" \
        $domain_args \
        --path="/var/lib/lego" \
        "${extra_args[@]}"
}

if [[ "$MODE" == "issue" ]]; then
    echo "🔹 [Lego] 正在申请首次证书: $DEBOPTI_DOMAINS ..."
else
    echo "🔹 [Lego] 正在检测/续期证书: $DEBOPTI_DOMAINS ..."
fi

if [[ $EUID -eq 0 ]]; then
    _run_lego
elif command -v sudo >/dev/null 2>&1; then
    # shellcheck disable=SC2086
    sudo /usr/local/bin/lego run \
        --env-file="$ENV_FILE" \
        --email="$DEBOPTI_EMAIL" \
        --dns="$DEBOPTI_PROVIDER" \
        $domain_args \
        --path="/var/lib/lego" \
        "${extra_args[@]}"
else
    echo "❌ [Lego] 需要 root 权限写入 /var/lib/lego，请以 root 或使用 sudo 运行 debopti。" >&2
    exit 1
fi

if [[ -f "$cert_path" ]]; then
    echo "✨ [Lego] 证书操作完成: $primary_domain"
else
    echo "❌ [Lego] 未找到预期证书文件: $cert_path" >&2
    exit 1
fi
