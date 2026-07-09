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
HOOK_DOMAIN_HINT="${1:-${LEGO_HOOK_CERT_NAME:-}}"
HOOK_DOMAIN_HINT="$(_lego_trim_string "$HOOK_DOMAIN_HINT")"

if ! _lego_resolve_hook_env_file "$HOOK_DOMAIN_HINT"; then
    echo "❌ [Lego Hook] 未找到对应的配置文件（需 LEGO_HOOK_CERT_NAME / LEGO_HOOK_CERT_DOMAINS 或命令行参数）。" >&2
    exit 1
fi

# shellcheck disable=SC1090
source "$ENV_FILE"

if ! _lego_validate_env_config; then
    exit 1
fi

if ! _lego_should_push_ferron; then
    exit 0
fi

CRT_SRC="${LEGO_HOOK_CERT_PATH:-/var/lib/lego/certificates/${primary_domain}.crt}"
KEY_SRC="${LEGO_HOOK_CERT_KEY_PATH:-/var/lib/lego/certificates/${primary_domain}.key}"

_lego_push_certs_to_ferron "$primary_domain" "$CRT_SRC" "$KEY_SRC"
