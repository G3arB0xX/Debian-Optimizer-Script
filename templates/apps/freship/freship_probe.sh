#!/bin/bash
# FreshIP 三核区域探针：Google 跳转 + YouTube Premium + YouTube Music

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

CONFIG_FILE="/etc/freship/freship.conf"
[[ -f "$CONFIG_FILE" ]] || exit 3
# shellcheck source=/dev/null
source "$CONFIG_FILE"

CORE_DIR="${INSTALL_DIR}/core"
# shellcheck source=/dev/null
source "${CORE_DIR}/freship_lib.sh"

INSTANCE_MODE=${INSTANCE_MODE:-${1:-v4}}
BIND_IP=""
[[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="${BIND_IPV4:-}"
[[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="${BIND_IPV6:-}"

build_bind_args

PROBE_UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"

extract_yt_gl() {
    grep -Eo '"(contentRegion|countryCode|INNERTUBE_CONTEXT_GL|GL)":"[A-Za-z]{2}"' | head -n 1 | cut -d'"' -f4 | tr 'a-z' 'A-Z'
}

JUMP_HDR=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -m 10 -sI -A "$PROBE_UA" "http://www.google.com/" 2>/dev/null || true)
JUMP_LOC=$(echo "$JUMP_HDR" | grep -i '^location:' | tr -d '\r\n')
JUMP_GL=$(parse_jump_gl "$JUMP_LOC")

YT_PR_GL=""
YT_PR_HTML=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -m 12 -s -L -A "$PROBE_UA" "https://www.youtube.com/premium" 2>/dev/null || true)
if [[ "$YT_PR_HTML" == *"www.google.cn"* ]]; then
    YT_PR_GL="CN"
else
    YT_PR_GL=$(printf '%s' "$YT_PR_HTML" | extract_yt_gl)
fi

YT_MU_GL=""
YT_MU_HTML=$(curl "${CURL_BIND_ARGS[@]}" "$DYNAMIC_IP_PREF" -m 12 -s -L -A "$PROBE_UA" "https://music.youtube.com/" 2>/dev/null || true)
if [[ "$YT_MU_HTML" == *"www.google.cn"* ]]; then
    YT_MU_GL="CN"
else
    YT_MU_GL=$(printf '%s' "$YT_MU_HTML" | extract_yt_gl)
fi

evaluate_probe_score "$JUMP_GL" "$YT_PR_GL" "$YT_MU_GL"
PROBE_RC=$?

now_utc=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
freship_write_state "LAST_PROBE_UTC" "$now_utc"
freship_write_state "LAST_SCORE" "$PROBE_SCORE"
freship_write_state "LAST_SCORE_MSG" "$PROBE_MSG"

if [[ "$PROBE_SCORE" == "ok" ]]; then
    freship_write_state "RUN_MODE" "maintain"
    [[ "${FRESHIP_PROBE_QUIET:-}" != "1" ]] && freship_log "PROBE" "SCORE" "OK | 区域自检通过 | ${PROBE_MSG}"
else
    freship_write_state "RUN_MODE" "simulate"
    if [[ "${FRESHIP_PROBE_QUIET:-}" != "1" ]]; then
        case "$PROBE_SCORE" in
            cn) freship_log "PROBE" "SCORE" "CN | 送中 | ${PROBE_MSG}" ;;
            drift) freship_log "PROBE" "SCORE" "DRIFT | 区域漂移 | ${PROBE_MSG}" ;;
            *) freship_log "PROBE" "SCORE" "FAIL | 探针异常 | ${PROBE_MSG}" ;;
        esac
    fi
fi

exit "$PROBE_RC"
