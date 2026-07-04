#!/bin/bash
# FreshIP Trust 模块：白名单访问、Cookie、泊松停留

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

mapfile -t TRUST_URLS < <(jq -r '.trust_module.white_urls[]? // empty' "$REGION_JSON" 2>/dev/null)
if [[ ${#TRUST_URLS[@]} -eq 0 ]]; then
    mapfile -t TRUST_URLS < <(jq -r '.trust_module.static_urls[]? // empty' "$REGION_JSON" 2>/dev/null)
fi
if [[ ${#TRUST_URLS[@]} -eq 0 ]]; then
    TRUST_URLS=(
        "https://en.wikipedia.org/wiki/Special:Random"
        "https://www.apple.com/"
        "https://www.microsoft.com/"
    )
fi

persona_from_ip "$BIND_IP" "$UA_FILE"
CURRENT_UA="$PERSONA_UA"

COOKIE_FILE=$(cookie_file_for "trust")
mkdir -p "${INSTALL_DIR}/data/cookies"
if ! acquire_cookie_lock "$COOKIE_FILE"; then
    freship_log "TRUST" "WARN" "已有 Trust 会话运行，跳过"
    exit 0
fi
trap 'release_cookie_lock' EXIT

STEP_COUNT=$(( RANDOM % 4 + 3 ))
SUCCESS_INJECT=0

for ((i = 1; i <= STEP_COUNT; i++)); do
    TARGET_URL="${TRUST_URLS[$((RANDOM % ${#TRUST_URLS[@]}))]}"
    http_code=""
    curl_rc=0
    http_code=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" \
        -b "$COOKIE_FILE" -c "$COOKIE_FILE" -A "$CURRENT_UA" \
        -H "Accept: text/html,application/xhtml+xml;q=0.9,image/avif,image/webp,*/*;q=0.8" \
        -H "Accept-Language: en-US,en;q=0.9" \
        -H "Sec-Fetch-Dest: document" \
        -H "Sec-Fetch-Mode: navigate" \
        -H "Sec-Fetch-Site: none" \
        -H "Upgrade-Insecure-Requests: 1" \
        --compressed \
        -s -L -o /dev/null -w '%{http_code}' -m 15 "$TARGET_URL" 2>/dev/null) || curl_rc=$?

    if [[ "$curl_rc" -ne 0 ]]; then
        http_code=$(map_curl_exit "$curl_rc")
        freship_log "TRUST" "ERROR" "[TRUST] 响应码: ${http_code} | TLS: ${BROWSE_TLS_MODE} | URL: ${TARGET_URL:0:40}..."
    elif [[ "$http_code" =~ ^[23] ]]; then
        freship_log "TRUST" "ACTION" "[TRUST] 响应码: ${http_code} | TLS: ${BROWSE_TLS_MODE} | URL: ${TARGET_URL:0:40}..."
        SUCCESS_INJECT=$((SUCCESS_INJECT + 1))
    else
        freship_log "TRUST" "ERROR" "[TRUST] 响应码: ${http_code} | TLS: ${BROWSE_TLS_MODE} | URL: ${TARGET_URL:0:40}..."
    fi

    if [[ "$i" -lt "$STEP_COUNT" ]]; then
        if [[ "${FRESHIP_TEST_FAST:-}" == "1" ]]; then
            sleep_time=1
        else
            sleep_time=$(poisson_sleep_seconds)
        fi
        sleep "$sleep_time"
    fi
done

exit 0
