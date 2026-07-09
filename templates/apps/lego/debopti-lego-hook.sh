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

# 载入配置
# shellcheck disable=SC1090
source "$ENV_FILE"

# 若开启了 Ferron 推送
if [[ "${DEBOPTI_FERRON_PUSH:-}" == "true" ]]; then
    if [[ -d "/etc/ferron" ]] || command -v ferron >/dev/null 2>&1; then
        echo "🔹 [Lego Hook] 检测到 Ferron，开始分发证书并设定高安全权限..."

        CRT_SRC="${LEGO_HOOK_CERT_PATH:-/var/lib/lego/certificates/${PRIMARY_DOMAIN}.crt}"
        KEY_SRC="${LEGO_HOOK_CERT_KEY_PATH:-/var/lib/lego/certificates/${PRIMARY_DOMAIN}.key}"

        if [[ ! -f "$CRT_SRC" || ! -f "$KEY_SRC" ]]; then
            echo "❌ [Lego Hook] 找不到已生成的证书文件: $CRT_SRC" >&2
            exit 1
        fi

        # 目标证书路径标准化
        mkdir -p /etc/ferron/certs
        install -m 644 -o ferron -g ferron "$CRT_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt" 2>/dev/null \
            || cp "$CRT_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt"
        install -m 600 -o ferron -g ferron "$KEY_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.key" 2>/dev/null \
            || cp "$KEY_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.key"

        chown -R ferron:ferron /etc/ferron/certs 2>/dev/null || true
        chmod 700 /etc/ferron/certs
        chmod 600 "/etc/ferron/certs/${PRIMARY_DOMAIN}.key"
        chmod 644 "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt"

        # 优雅重载/重启 Web 服务
        if systemctl is-active --quiet ferron; then
            systemctl reload-or-restart ferron 2>/dev/null || systemctl restart ferron
            echo "✨ [Lego Hook] Ferron 服务已成功载入新证书并重载运行。"
        fi
    else
        echo "⚠️ [Lego Hook] 开启了 Ferron 推送但系统未安装 Ferron 服务，略过。"
    fi
fi
