#!/bin/bash
# =========================================================
# Lego 证书自动推送与服务重载钩子脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

if [[ ! -f /usr/local/bin/debopti-lego-lib.sh ]]; then
    echo "❌ [Lego Hook] 缺少 debopti-lego-lib.sh，请重新安装 Lego。" >&2
    exit 1
fi

# shellcheck source=/dev/null
source /usr/local/bin/debopti-lego-lib.sh

# Lego v5 deploy-hook 通过 LEGO_HOOK_* 注入证书信息；手动调用时可传主域名参数
PRIMARY_DOMAIN="${1:-${LEGO_HOOK_CERT_NAME:-}}"
PRIMARY_DOMAIN="${PRIMARY_DOMAIN#"${PRIMARY_DOMAIN%%[![:space:]]*}"}"
PRIMARY_DOMAIN="${PRIMARY_DOMAIN%"${PRIMARY_DOMAIN##*[![:space:]]}"}"

if ! _lego_is_safe_primary_domain "$PRIMARY_DOMAIN"; then
    echo "❌ [Lego Hook] 无法确定有效主域名（需 LEGO_HOOK_CERT_NAME 或命令行参数）。" >&2
    exit 1
fi

ENV_FILE="/etc/lego/envs/${PRIMARY_DOMAIN}.env"
if [[ ! -f "$ENV_FILE" ]]; then
    echo "❌ [Lego Hook] 未找到对应的配置文件: $ENV_FILE" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

if ! _lego_should_push_ferron; then
    exit 0
fi

CRT_SRC="${LEGO_HOOK_CERT_PATH:-/var/lib/lego/certificates/${PRIMARY_DOMAIN}.crt}"
KEY_SRC="${LEGO_HOOK_CERT_KEY_PATH:-/var/lib/lego/certificates/${PRIMARY_DOMAIN}.key}"

_lego_push_certs_to_ferron "$PRIMARY_DOMAIN" "$CRT_SRC" "$KEY_SRC"
