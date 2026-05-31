#!/bin/bash
export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
CONFIG_FILE="/etc/freship/freship.conf"
[[ ! -f "$CONFIG_FILE" ]] && exit 1
source "$CONFIG_FILE"
INSTANCE_MODE=${1:-"global"}
DATA_DIR="${INSTALL_DIR}/data"
REGION_JSON=$(find "${DATA_DIR}/regions" -name "*.json" | head -n 1)

log() {
    local type=$1; local msg=$2; local icon="ℹ️"
    case "$type" in START) icon="🚀" ;; INFO) icon="📊" ;; SLEEP) icon="🌙" ;; SUCCESS) icon="✅" ;; ERROR) icon="❌" ;; ACTION) icon="🔗" ;; esac
    echo -e "[FreshIP] $icon | $(date '+%Y-%m-%d %H:%M:%S') | $INSTANCE_MODE | $REGION_CODE | $msg"
}

LOCAL_HOUR=$(date -u -d "${UTC_OFFSET:-+0} hours" +%H)
if [ "$LOCAL_HOUR" -ge 1 ] && [ "$LOCAL_HOUR" -le 6 ]; then
    log "SLEEP" "处于目标地区深夜 ($LOCAL_HOUR:00)，进入休眠模式。"
    exit 0
fi

DAILY_SEED=$(echo $(date +%Y%m%d) | cksum | awk '{print $1}')
ACTIVITY_LEVEL=$(( DAILY_SEED % 100 ))
if [ "$ACTIVITY_LEVEL" -lt 30 ] && [ $(( RANDOM % 100 )) -gt "$ACTIVITY_LEVEL" ]; then
    log "INFO" "今日活跃度低 ($ACTIVITY_LEVEL%)，当前轮次选择休假。"
    exit 0
fi

log "START" "启动养护任务 (活跃度: $ACTIVITY_LEVEL%)"
LOCK_FILE="/tmp/freship_${INSTANCE_MODE}.lock"; echo $$ > "$LOCK_FILE"; trap 'rm -f "$LOCK_FILE"' EXIT
BIND_IP=""; [[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="$BIND_IPV4"; [[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="$BIND_IPV6"
[[ -z "$BIND_IP" ]] && exit 1

CURL_BIN="curl"; TLS_MODE="Native"
for candidate in "curl_chrome124" "curl_chrome116" "curl_chrome110"; do
    if [ -f "${INSTALL_DIR}/bin/$candidate" ]; then CURL_BIN="${INSTALL_DIR}/bin/$candidate"; TLS_MODE="$candidate"; break; fi
done

UA_POOL="${DATA_DIR}/user_agents.txt"
SESSION_UA=$( [ -f "$UA_POOL" ] && shuf -n 1 "$UA_POOL" || echo "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/124.0.0.0 Safari/537.36" )
IS_MOBILE=false; [[ "$SESSION_UA" =~ "Android" || "$SESSION_UA" =~ "iPhone" ]] && IS_MOBILE=true
PREV_URL=""

request() {
    local url=$1; local name=$2; local display_info=${3:-"URL: ${url:0:40}..."}
    local site_header="none"; [[ -n "$PREV_URL" ]] && site_header="same-origin"
    local cmd=( "$CURL_BIN" -s -L -o /dev/null -w "%{http_code}" --interface "$BIND_IP" -A "$SESSION_UA" )
    [[ -n "$PREV_URL" ]] && cmd+=( -e "$PREV_URL" ); cmd+=( -H "Sec-Fetch-Site: $site_header" )
    local code=$( "${cmd[@]}" "$url" )
    if [[ "$code" =~ ^2 ]]; then log "ACTION" "[$name] 响应码: $code | TLS: $TLS_MODE | $display_info"; else log "ERROR" "[$name] 响应码: $code | TLS: $TLS_MODE | $display_info"; fi
    PREV_URL="$url"; sleep $(( RANDOM % 5 + 2 ))
}

ROLL=$(( RANDOM % 100 ))
if [ "$ROLL" -lt 60 ]; then
    KW_FILE="${DATA_DIR}/keywords/kw_${REGION_CODE}.txt"; KW=$( [ -f "$KW_FILE" ] && shuf -n 1 "$KW_FILE" || echo "Debian Linux" )
    ENCODED_KW=$(jq -rn --arg x "$KW" '$x|@uri')
    request "https://www.google.com/search?q=${ENCODED_KW}" "SEARCH" "关键字: $KW"
elif [ "$ROLL" -lt 85 ]; then
    if [ $(( RANDOM % 100 )) -lt 70 ]; then URL=$( jq -r '.trust_module.white_urls[]' "$REGION_JSON" | shuf -n 1 ); request "$URL" "NEWS_WHITE"
    else URL=$( jq -r '.trust_module.static_urls[]' "$REGION_JSON" | shuf -n 1 ); request "$URL" "NEWS_STATIC"; fi
elif [ "$ROLL" -lt 95 ]; then request "https://www.google.com/maps/search/restaurants+near+me" "MAPS"
else if [ "$IS_MOBILE" = true ]; then request "http://connectivitycheck.gstatic.com/generate_204" "PROBE_MOBILE"
    else request "https://www.google.com/imghp?hl=zh-CN" "PROBE_DESKTOP"; fi
fi
log "SUCCESS" "养护流程执行完毕。"
