#!/bin/bash
# =========================================================
# Lego 证书自动推送与服务重载钩子脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

PRIMARY_DOMAIN="${1:-}"
if [[ -z "$PRIMARY_DOMAIN" ]]; then
    echo "❌ [Lego Hook] 缺少参数: 主域名。" >&2
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
        
        CRT_SRC="/var/lib/lego/certificates/${PRIMARY_DOMAIN}.crt"
        KEY_SRC="/var/lib/lego/certificates/${PRIMARY_DOMAIN}.key"
        
        if [[ ! -f "$CRT_SRC" || ! -f "$KEY_SRC" ]]; then
            echo "❌ [Lego Hook] 找不到已生成的证书文件: $CRT_SRC" >&2
            exit 1
        fi
        
        # 目标证书路径标准化
        mkdir -p /etc/ferron/certs
        cp "$CRT_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.crt"
        cp "$KEY_SRC" "/etc/ferron/certs/${PRIMARY_DOMAIN}.key"
        
        # 权限安全加固：私钥 600, 公钥 644, 目录 700
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
