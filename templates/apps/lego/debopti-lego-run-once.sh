#!/bin/bash
# =========================================================
# Lego 单域名证书申请/续期脚本 (Debopti 托管)
# 用法: debopti-lego-run-once.sh <env_file> [issue|renew|auto]
# 手动模式: DEBOPTI_INTERACTIVE_LEG=1（TUI 已设置）支持按 s 跳过 / 失败后 y/N 重试
# =========================================================
set -euo pipefail

if [[ ! -f /usr/local/bin/debopti-lego-lib.sh ]]; then
    echo "❌ [Lego] 缺少 debopti-lego-lib.sh，请重新安装 Lego。" >&2
    exit 1
fi

# shellcheck source=/dev/null
source /usr/local/bin/debopti-lego-lib.sh

MODE="${2:-auto}"

if ! _lego_assert_env_file "${1:-}"; then
    echo "❌ [Lego] 缺少或无效的环境配置文件（仅允许 /etc/lego/envs/*.env）。" >&2
    exit 1
fi

if [[ "$MODE" != "issue" && "$MODE" != "renew" && "$MODE" != "auto" ]]; then
    echo "❌ [Lego] 无效模式: $MODE（允许: issue, renew, auto）" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

if ! _lego_validate_env_config; then
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

_lego_prepare_runtime_dirs

if [[ "$MODE" == "issue" ]]; then
    echo "🔹 [Lego] 正在申请首次证书: $DEBOPTI_DOMAINS ..."
else
    echo "🔹 [Lego] 正在检测/续期证书: $DEBOPTI_DOMAINS ..."
fi

if [[ "${DEBOPTI_DNS_SKIP_PROPAGATION:-}" == "true" ]]; then
    echo "ℹ️ [Lego] 已配置跳过 DNS 传播校验"
fi

set +e
_lego_run_certificate_flow "$MODE"
flow_rc=$?
set -e

if [[ $flow_rc -eq 0 && -f "$cert_path" ]]; then
    echo "✨ [Lego] 证书操作完成: $primary_domain"
    exit 0
fi

if [[ $flow_rc -eq 0 && ! -f "$cert_path" ]]; then
    echo "❌ [Lego] Lego 返回成功但未找到证书文件: $cert_path" >&2
    exit 1
fi

echo "❌ [Lego] 证书操作失败: $primary_domain" >&2
exit 1
