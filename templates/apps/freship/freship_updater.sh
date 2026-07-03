#!/bin/bash
# FreshIP 热数据同步：keywords、region、map、UA 池

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

CONFIG_FILE="/etc/freship/freship.conf"
[[ -f "$CONFIG_FILE" ]] || exit 1
# shellcheck source=/dev/null
source "$CONFIG_FILE"

CORE_DIR="${INSTALL_DIR}/core"
# shellcheck source=/dev/null
source "${CORE_DIR}/freship_lib.sh"

INSTANCE_MODE="v4"
BIND_IP="${BIND_IPV4:-}"
[[ -z "$BIND_IP" && -n "${BIND_IPV6:-}" ]] && { BIND_IP="$BIND_IPV6"; INSTANCE_MODE="v6"; }
build_bind_args

UA_TIME_FILE="${INSTALL_DIR}/core/.ua_last_update"
NOW=$(date +%s)
LAST_UPDATE=0
[[ -f "$UA_TIME_FILE" ]] && LAST_UPDATE=$(tr -d '\r\n' < "$UA_TIME_FILE")
[[ ! "$LAST_UPDATE" =~ ^[0-9]+$ ]] && LAST_UPDATE=0
DIFF=$((NOW - LAST_UPDATE))

curl_fetch() {
    local dest=$1 url=$2
    local tmp="${dest}.tmp.$$"
    if curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -fsSL -m 30 "$url" -o "$tmp" 2>/dev/null && [[ -s "$tmp" ]]; then
        mv "$tmp" "$dest"
        return 0
    fi
    rm -f "$tmp"
    return 1
}

freship_log "UPDATER" "START" "热数据同步开始"

kw_name="${KW_FILE:-kw_${REGION_CODE}.txt}"
if curl_fetch "${INSTALL_DIR}/data/keywords/${kw_name}" "${FRESHIP_REPO_RAW}/data/keywords/${kw_name}"; then
    freship_log "UPDATER" "INFO" "关键词 ${kw_name} 已更新"
else
    freship_log "UPDATER" "WARN" "关键词 ${kw_name} 拉取失败，保留本地"
fi

if [[ -n "${REGION_PATH:-}" ]]; then
    region_dest="${INSTALL_DIR}/data/regions/${REGION_PATH}.json"
    mkdir -p "$(dirname "$region_dest")"
    if curl_fetch "$region_dest" "${FRESHIP_REPO_RAW}/data/regions/${REGION_PATH}.json"; then
        freship_log "UPDATER" "INFO" "region ${REGION_PATH} 已更新"
    else
        freship_log "UPDATER" "WARN" "region ${REGION_PATH} 拉取失败，保留本地"
    fi
fi

if curl_fetch "${INSTALL_DIR}/data/map.json" "${FRESHIP_REPO_RAW}/data/map.json"; then
    freship_log "UPDATER" "INFO" "map.json 已更新"
else
    freship_log "UPDATER" "WARN" "map.json 拉取失败，保留本地"
fi

if [[ "$DIFF" -ge 2592000 || "$LAST_UPDATE" -eq 0 ]]; then
    if curl_fetch "${INSTALL_DIR}/data/user_agents.txt" "${FRESHIP_REPO_RAW}/data/user_agents.txt"; then
        echo "$NOW" > "$UA_TIME_FILE"
        freship_log "UPDATER" "INFO" "UA 池已更新 (30 天周期)"
    else
        freship_log "UPDATER" "WARN" "UA 池拉取失败，保留本地"
    fi
else
    days_left=$(( (2592000 - DIFF) / 86400 ))
    freship_log "UPDATER" "INFO" "UA 池静默期，约 ${days_left} 天后更新"
fi

if [[ -f "${LOG_FILE:-}" ]]; then
    tail -n 2000 "${LOG_FILE}" > "${LOG_FILE}.tmp" 2>/dev/null && mv "${LOG_FILE}.tmp" "${LOG_FILE}"
fi

freship_log "UPDATER" "END" "热数据同步结束"
exit 0
