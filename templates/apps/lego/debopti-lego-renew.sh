#!/bin/bash
# =========================================================
# Lego 证书定时检测与自动续期主脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

if [[ ! -f /usr/local/bin/debopti-lego-lib.sh ]]; then
    echo "❌ [Lego] 缺少 debopti-lego-lib.sh，请重新安装 Lego。" >&2
    exit 1
fi

# shellcheck source=/dev/null
source /usr/local/bin/debopti-lego-lib.sh

if [[ ! -x /usr/local/bin/lego ]]; then
    echo "❌ [Lego] 未找到 /usr/local/bin/lego，请先安装 Lego。" >&2
    exit 1
fi

ENV_DIR="/etc/lego/envs"
if [[ ! -d "$ENV_DIR" ]]; then
    exit 0
fi

shopt -s nullglob
env_files=("$ENV_DIR"/*.env)
shopt -u nullglob

for env_file in "${env_files[@]}"; do
    [[ -f "$env_file" ]] || continue

    # 局部载入环境变量，防止变量污染；单域名失败不中断其他域名
    if ! (
        set -euo pipefail
        ENV_FILE=""
        if ! _lego_assert_env_file "$env_file"; then
            echo "❌ [Lego] 跳过无效配置文件: $env_file" >&2
            exit 0
        fi

        # shellcheck disable=SC1090
        source "$ENV_FILE"

        if [[ "${DEBOPTI_AUTO_RENEW:-}" != "true" ]]; then
            exit 0
        fi

        if ! _lego_validate_env_config; then
            echo "❌ [Lego] 配置无效，跳过: $ENV_FILE" >&2
            exit 0
        fi

        cert_path="/var/lib/lego/certificates/${primary_domain}.crt"
        if [[ ! -f "$cert_path" ]]; then
            echo "⚠️ [Lego] 证书尚未申请，跳过续期: $primary_domain"
            exit 0
        fi

        echo "🔹 [Lego] 开始检测/更新证书: $DEBOPTI_DOMAINS ..."
        if [[ "${DEBOPTI_DNS_SKIP_PROPAGATION:-}" == "true" ]]; then
            echo "ℹ️ [Lego] 已配置跳过 DNS 传播校验（自动续期）"
        fi

        # Lego v5.2+：run 统一负责续期；--renew-days 30 表示剩余 30 天内才实际续签
        if _lego_run_renew_once; then
            echo "✨ [Lego] 证书检测完成: $primary_domain"
        else
            echo "❌ [Lego] 证书续期失败: $primary_domain" >&2
            exit 1
        fi
    ); then
        : # 单域名失败已在子 shell 内记录日志
    fi
done
