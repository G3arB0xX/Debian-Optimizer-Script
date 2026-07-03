#!/bin/bash
# FreshIP Google 模块：多步会话、Persona、Cookie、坐标抖动、Referer 域隔离

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

CONFIG_FILE="/etc/freship/freship.conf"
[[ -f "$CONFIG_FILE" ]] || exit 1
# shellcheck source=/dev/null
source "$CONFIG_FILE"

CORE_DIR="${INSTALL_DIR}/core"
# shellcheck source=/dev/null
source "${CORE_DIR}/freship_lib.sh"

INSTANCE_MODE=${INSTANCE_MODE:-v4}
BIND_IP=""
[[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="${BIND_IPV4:-}"
[[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="${BIND_IPV6:-}"
[[ -z "$BIND_IP" ]] && exit 1

build_bind_args
resolve_browse_curl

REGION_JSON=$(region_json_path)
UA_FILE="${INSTALL_DIR}/data/user_agents.txt"
KW_PATH="${INSTALL_DIR}/data/keywords/${KW_FILE:-kw_${REGION_CODE}.txt}"

if [[ ! -f "$REGION_JSON" || ! -f "$KW_PATH" ]]; then
    freship_log "GOOGLE" "ERROR" "region JSON 或关键词文件缺失"
    exit 1
fi

BASE_LAT=$(jq -r '.google_module.base_lat // empty' "$REGION_JSON" 2>/dev/null)
BASE_LON=$(jq -r '.google_module.base_lon // empty' "$REGION_JSON" 2>/dev/null)
LANG_PARAMS=$(jq -r '.google_module.lang_params // empty' "$REGION_JSON" 2>/dev/null)
LANG_HL=$(lang_hl_from_params "$LANG_PARAMS")

if ! [[ "$BASE_LAT" =~ ^-?[0-9]+(\.[0-9]+)?$ ]] || ! [[ "$BASE_LON" =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then
    freship_log "GOOGLE" "ERROR" "区域坐标缺失或非法"
    exit 1
fi

mapfile -t KEYWORDS < <(grep -v '^[[:space:]]*$' "$KW_PATH")
if [[ ${#KEYWORDS[@]} -eq 0 ]]; then
    freship_log "GOOGLE" "ERROR" "关键词池为空"
    exit 1
fi

persona_from_ip "$BIND_IP" "$UA_FILE"
SESSION_UA="$PERSONA_UA"
UA_PLATFORM=$(ua_platform_from_string "$SESSION_UA")

SESSION_BASE_LAT=$(get_random_coord "$BASE_LAT" 270)
SESSION_BASE_LON=$(get_random_coord "$BASE_LON" 270)
TOTAL_ACTIONS=$(( 5 + RANDOM % 4 ))

COOKIE_FILE=$(cookie_file_for "google")
mkdir -p "${INSTALL_DIR}/data/cookies"
if ! acquire_cookie_lock "$COOKIE_FILE"; then
    freship_log "GOOGLE" "WARN" "已有 Google 会话运行，跳过"
    exit 0
fi
trap 'release_cookie_lock' EXIT

freship_log "GOOGLE" "START" "会话开始 | 动作数: ${TOTAL_ACTIONS} | TLS: ${BROWSE_TLS_MODE} | 平台: ${UA_PLATFORM}"
freship_log "GOOGLE" "INFO" "坐标驻留: ${SESSION_BASE_LAT}, ${SESSION_BASE_LON}"

REF_SEARCH=""
REF_NEWS=""
REF_MAPS=""
REF_ECO=""

for ((i = 1; i <= TOTAL_ACTIONS; i++)); do
    ACTION_LAT=$(get_random_coord "$SESSION_BASE_LAT" 1)
    ACTION_LON=$(get_random_coord "$SESSION_BASE_LON" 1)
    RAND_KEY="${KEYWORDS[$((RANDOM % ${#KEYWORDS[@]}))]}"
    ENCODED_KEY=$(encode_uri_component "$RAND_KEY")
    [[ -z "$ENCODED_KEY" ]] && ENCODED_KEY=$(printf '%s' "$RAND_KEY" | tr ' ' '+')

    ACTION_DICE=$(( RANDOM % 100 ))
    TARGET_URL=""
    ACTION_LOG=""

    if [[ "$UA_PLATFORM" == "android" ]]; then
        if [[ "$ACTION_DICE" -lt 25 ]]; then
            TARGET_URL="https://www.google.com/search?q=${ENCODED_KEY}&${LANG_PARAMS}"
            ACTION_LOG="Search"
        elif [[ "$ACTION_DICE" -lt 55 ]]; then
            TARGET_URL="https://news.google.com/home?${LANG_PARAMS}"
            ACTION_LOG="News"
        elif [[ "$ACTION_DICE" -lt 85 ]]; then
            TARGET_URL="https://www.google.com/maps/search/${ENCODED_KEY}/@${ACTION_LAT},${ACTION_LON},17z?${LANG_PARAMS}"
            ACTION_LOG="Maps"
        else
            TARGET_URL="https://connectivitycheck.gstatic.com/generate_204"
            ACTION_LOG="NetTest"
        fi
    elif [[ "$UA_PLATFORM" == "ios" || "$UA_PLATFORM" == "macos" ]]; then
        if [[ "$ACTION_DICE" -lt 30 ]]; then
            TARGET_URL="https://www.google.com/search?q=${ENCODED_KEY}&${LANG_PARAMS}"
            ACTION_LOG="Search"
        elif [[ "$ACTION_DICE" -lt 65 ]]; then
            TARGET_URL="https://news.google.com/home?${LANG_PARAMS}"
            ACTION_LOG="News"
        elif [[ "$ACTION_DICE" -lt 90 ]]; then
            TARGET_URL="https://www.google.com/maps/search/${ENCODED_KEY}/@${ACTION_LAT},${ACTION_LON},17z?${LANG_PARAMS}"
            ACTION_LOG="Maps"
        else
            TARGET_URL="https://captive.apple.com/hotspot-detect.html"
            ACTION_LOG="NetTest"
        fi
    else
        if [[ "$ACTION_DICE" -lt 20 ]]; then
            TARGET_URL="https://www.google.com/search?q=${ENCODED_KEY}&${LANG_PARAMS}"
            ACTION_LOG="Search"
        elif [[ "$ACTION_DICE" -lt 60 ]]; then
            TARGET_URL="https://news.google.com/home?${LANG_PARAMS}"
            ACTION_LOG="News"
        elif [[ "$ACTION_DICE" -lt 80 ]]; then
            eco_urls=(
                "https://about.google/"
                "https://safety.google/"
                "https://support.google.com/?hl=${LANG_HL}"
            )
            TARGET_URL="${eco_urls[$((RANDOM % ${#eco_urls[@]}))]}"
            ACTION_LOG="EcoRoam"
        else
            TARGET_URL="https://www.google.com/maps/search/${ENCODED_KEY}/@${ACTION_LAT},${ACTION_LON},17z?${LANG_PARAMS}"
            ACTION_LOG="Maps"
        fi
    fi

    CTX_REF=""
    case "$ACTION_LOG" in
        Search) CTX_REF="$REF_SEARCH" ;;
        News) CTX_REF="$REF_NEWS" ;;
        Maps) CTX_REF="$REF_MAPS" ;;
        EcoRoam) CTX_REF="$REF_ECO" ;;
    esac

    code=""
    curl_rc=0
    if [[ -n "$CTX_REF" && $((RANDOM % 100)) -lt 70 ]]; then
        code=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -m 15 -s -L -o /dev/null -w '%{http_code}' \
            -b "$COOKIE_FILE" -c "$COOKIE_FILE" -A "$SESSION_UA" -H "Referer: $CTX_REF" "$TARGET_URL" 2>/dev/null) || curl_rc=$?
    else
        code=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -m 15 -s -L -o /dev/null -w '%{http_code}' \
            -b "$COOKIE_FILE" -c "$COOKIE_FILE" -A "$SESSION_UA" "$TARGET_URL" 2>/dev/null) || curl_rc=$?
    fi

    if [[ "$curl_rc" -ne 0 ]]; then
        code=$(map_curl_exit "$curl_rc")
        freship_log "GOOGLE" "WARN" "动作[${i}/${TOTAL_ACTIONS}] ${ACTION_LOG} | ${code} | ${ACTION_LAT}, ${ACTION_LON}"
        case "$ACTION_LOG" in
            Search) REF_SEARCH="" ;;
            News) REF_NEWS="" ;;
            Maps) REF_MAPS="" ;;
            EcoRoam) REF_ECO="" ;;
        esac
    elif [[ "$code" =~ ^[23] ]]; then
        freship_log "GOOGLE" "EXEC" "动作[${i}/${TOTAL_ACTIONS}] ${ACTION_LOG} | HTTP ${code} | ${ACTION_LAT}, ${ACTION_LON}"
        case "$ACTION_LOG" in
            Search) REF_SEARCH="$TARGET_URL" ;;
            News) REF_NEWS="$TARGET_URL" ;;
            Maps) REF_MAPS="$TARGET_URL" ;;
            EcoRoam) REF_ECO="$TARGET_URL" ;;
        esac
    else
        freship_log "GOOGLE" "WARN" "动作[${i}/${TOTAL_ACTIONS}] ${ACTION_LOG} | HTTP ${code}"
    fi

    if [[ "$i" -lt "$TOTAL_ACTIONS" ]]; then
        if [[ "${FRESHIP_TEST_FAST:-}" == "1" ]]; then
            sleep_time=1
        else
            sleep_time=$(( 45 + RANDOM % 31 ))
        fi
        freship_log "GOOGLE" "WAIT" "停留 ${sleep_time}s"
        sleep "$sleep_time"
    fi
done

freship_log "GOOGLE" "END" "会话结束"
exit 0
