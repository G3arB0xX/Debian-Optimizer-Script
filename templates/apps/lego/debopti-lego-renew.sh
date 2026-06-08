#!/bin/bash
# =========================================================
# Lego 证书定时检测与自动续期主脚本 (Debopti 托管)
# =========================================================
set -euo pipefail

ENV_DIR="/etc/lego/envs"
if [[ ! -d "$ENV_DIR" ]]; then
    exit 0
fi

# 轮询所有域名的环境配置文件
for env_file in "$ENV_DIR"/*.env; do
    [[ ! -f "$env_file" ]] && continue
    
    # 局部载入环境变量，防止变量污染
    (
        # shellcheck disable=SC1090
        source "$env_file"
        
        # 仅对开启自动更新的证书执行
        if [[ "${DEBOPTI_AUTO_RENEW:-}" == "true" ]]; then
            echo "🔹 [Lego] 开始检测/更新证书: $DEBOPTI_DOMAINS ..."
            
            # 解析域名列表为 lego 格式参数 (例如 "a.com,b.com" -> --domains=a.com --domains=b.com)
            domain_args=""
            IFS=',' read -ra ADDR <<< "$DEBOPTI_DOMAINS"
            for d in "${ADDR[@]}"; do
                domain_args="$domain_args --domains=$d"
            done
            
            # 首个域名作为主域名
            primary_domain="${ADDR[0]}"
            
            # 执行静默续期 (到期 30 天内才会实际触发申请)
            # shellcheck disable=SC2086
            if /usr/local/bin/lego --email="$DEBOPTI_EMAIL" \
                             --dns="$DEBOPTI_PROVIDER" \
                             $domain_args \
                             --path="/var/lib/lego" \
                             --accept-tos \
                             renew --days 30 \
                             --renew-hook "/usr/local/bin/debopti-lego-hook.sh $primary_domain"; then
                echo "✨ [Lego] 证书检测完成: $primary_domain"
            else
                echo "❌ [Lego] 证书续期失败: $primary_domain"
            fi
        fi
    )
done
