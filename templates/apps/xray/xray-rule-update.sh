#!/bin/bash
# Xray 规则自动更新脚本 (支持地区自适应与多规则集隔离)
ASSET_DIR="/usr/local/share/xray"
IS_CN="{{IS_CN}}"

CONFIG_FILE="/etc/debopti/debopti.conf"
ACTIVE_RULESET="official"
if [[ -f "$CONFIG_FILE" ]]; then
    ACTIVE_RULESET=$(grep -E "^XRAY_RULESET=" "$CONFIG_FILE" | cut -d'=' -f2- | tr -d '"'\'' ' || echo "official")
fi

# 仅在当前处于第三方规则集模式下执行更新
if [[ "$ACTIVE_RULESET" != "loyalsoldier" ]]; then
    exit 0
fi

GEOSITE_URL="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat"
GEOIP_URL="https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat"

GEOSITE_MIRROR="https://ghfast.top/${GEOSITE_URL}"
GEOIP_MIRROR="https://ghfast.top/${GEOIP_URL}"

if [[ "$IS_CN" == "true" ]]; then
    GEOSITE_URL="${GEOSITE_MIRROR}"
    GEOIP_URL="${GEOIP_MIRROR}"
fi

UPDATED=false

# 优先拉取 Geosite (海外直连，失败回退镜像)
if curl -fsSL -m 60 -o "${ASSET_DIR}/geosite.dat.loyalsoldier.new" "$GEOSITE_URL" && [[ -s "${ASSET_DIR}/geosite.dat.loyalsoldier.new" ]]; then
    mv -f "${ASSET_DIR}/geosite.dat.loyalsoldier.new" "${ASSET_DIR}/geosite.dat.loyalsoldier"
    UPDATED=true
else
    if [[ "$IS_CN" != "true" ]]; then
        # 海外环境下，如果 GitHub 直连失败，降级使用镜像恢复
        if curl -fsSL -m 60 -o "${ASSET_DIR}/geosite.dat.loyalsoldier.new" "${GEOSITE_MIRROR}" && [[ -s "${ASSET_DIR}/geosite.dat.loyalsoldier.new" ]]; then
            mv -f "${ASSET_DIR}/geosite.dat.loyalsoldier.new" "${ASSET_DIR}/geosite.dat.loyalsoldier"
            UPDATED=true
        else
            rm -f "${ASSET_DIR}/geosite.dat.loyalsoldier.new"
        fi
    else
        rm -f "${ASSET_DIR}/geosite.dat.loyalsoldier.new"
    fi
fi

# 优先拉取 Geoip (海外直连，失败回退镜像)
if curl -fsSL -m 60 -o "${ASSET_DIR}/geoip.dat.loyalsoldier.new" "$GEOIP_URL" && [[ -s "${ASSET_DIR}/geoip.dat.loyalsoldier.new" ]]; then
    mv -f "${ASSET_DIR}/geoip.dat.loyalsoldier.new" "${ASSET_DIR}/geoip.dat.loyalsoldier"
    UPDATED=true
else
    if [[ "$IS_CN" != "true" ]]; then
        # 海外环境下，如果 GitHub 直连失败，降级使用镜像恢复
        if curl -fsSL -m 60 -o "${ASSET_DIR}/geoip.dat.loyalsoldier.new" "${GEOIP_MIRROR}" && [[ -s "${ASSET_DIR}/geoip.dat.loyalsoldier.new" ]]; then
            mv -f "${ASSET_DIR}/geoip.dat.loyalsoldier.new" "${ASSET_DIR}/geoip.dat.loyalsoldier"
            UPDATED=true
        else
            rm -f "${ASSET_DIR}/geoip.dat.loyalsoldier.new"
        fi
    else
        rm -f "${ASSET_DIR}/geoip.dat.loyalsoldier.new"
    fi
fi

if [[ "$UPDATED" == "true" ]]; then
    # 强制重新建立软链接以防外部篡改
    ln -sf geosite.dat.loyalsoldier "${ASSET_DIR}/geosite.dat"
    ln -sf geoip.dat.loyalsoldier "${ASSET_DIR}/geoip.dat"
    systemctl restart xray >/dev/null 2>&1
fi
