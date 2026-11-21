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

_xray_rule_fetch() {
    local url=$1
    local dest=$2
    local tmp_dest="${dest}.new"
    local proxies=(
        "https://gh-proxy.com"
        "https://cdn.gh-proxy.org"
        "https://edgeone.gh-proxy.org"
        "https://git.yylx.win"
        "https://gh-proxy.org"
        "https://ghfast.top"
        "https://wget.la"
    )
    local candidates=()
    local proxy candidate

    if [[ "$IS_CN" == "true" ]]; then
        for proxy in "${proxies[@]}"; do
            candidates+=("${proxy}/${url}")
        done
        candidates+=("$url")
    else
        candidates+=("$url")
        for proxy in "${proxies[@]}"; do
            candidates+=("${proxy}/${url}")
        done
    fi

    for candidate in "${candidates[@]}"; do
        if curl -fsSL --connect-timeout 10 -m 60 -o "$tmp_dest" "$candidate" && [[ -s "$tmp_dest" ]]; then
            mv -f "$tmp_dest" "$dest"
            return 0
        fi
        rm -f "$tmp_dest"
    done
    return 1
}

UPDATED=false

if _xray_rule_fetch "$GEOSITE_URL" "${ASSET_DIR}/geosite.dat.loyalsoldier"; then
    UPDATED=true
fi

if _xray_rule_fetch "$GEOIP_URL" "${ASSET_DIR}/geoip.dat.loyalsoldier"; then
    UPDATED=true
fi

if [[ "$UPDATED" == "true" ]]; then
    # 强制重新建立软链接以防外部篡改
    ln -sf geosite.dat.loyalsoldier "${ASSET_DIR}/geosite.dat"
    ln -sf geoip.dat.loyalsoldier "${ASSET_DIR}/geoip.dat"
    systemctl restart xray >/dev/null 2>&1
fi
