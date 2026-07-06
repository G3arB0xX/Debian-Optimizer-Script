#!/bin/bash
# FreshIP 公共库：日志、Persona、Cookie、坐标、curl 绑定

FRESHIP_REPO_RAW="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"

_freship_log_icon() {
    local level=$1 msg=$2 action_tag=${3:-}
    if [[ -n "$action_tag" ]]; then
        case "$action_tag" in
            SEARCH) echo "🔍" ;;
            NEWS) echo "📰" ;;
            MAPS) echo "🗺️" ;;
            ECO) echo "🌐" ;;
            NETTEST) echo "📡" ;;
            TRUST) echo "🔗" ;;
            *) echo "🔗" ;;
        esac
        return
    fi
    case "$level" in
        START) echo "🚀" ;;
        END|SUCCESS) echo "✅" ;;
        SLEEP) echo "🌙" ;;
        ERROR) echo "❌" ;;
        WARN) echo "⚠️" ;;
        SCORE)
            if [[ "$msg" == OK* || "$msg" == *"区域自检通过"* || "$msg" == *"区域达标"* ]]; then
                echo "✅"
            elif [[ "$msg" == *"送中"* || "$msg" == CN* ]]; then
                echo "❌"
            else
                echo "📊"
            fi
            ;;
        WAIT) echo "⏳" ;;
        INFO|*) echo "📊" ;;
    esac
}

freship_log() {
    local module=$1 level=$2 msg=$3 action_tag=${4:-}
    local icon ts file_line journal_line log_file
    if [[ "$level" == "ERROR" ]]; then
        icon=$(_freship_log_icon "ERROR" "$msg")
    else
        icon=$(_freship_log_icon "$level" "$msg" "$action_tag")
    fi
    ts=$(date '+%Y-%m-%d %H:%M:%S')
    file_line="${ts} [FreshIP] ${icon} | ${INSTANCE_MODE:-?} | ${REGION_CODE:-?} | ${msg}"
    journal_line="${icon} | ${INSTANCE_MODE:-?} | ${REGION_CODE:-?} | ${msg}"
    log_file="${LOG_FILE:-/opt/freship/logs/freship.log}"
    mkdir -p "$(dirname "$log_file")"
    echo "$file_line" >> "$log_file"
    if command -v logger >/dev/null 2>&1; then
        logger -t freship "${journal_line}"
    else
        echo "$file_line"
    fi
}

freship_state_file() {
    echo "/etc/freship/state/${INSTANCE_MODE}.state"
}

freship_write_state() {
    local key=$1 val=$2
    local sf
    sf=$(freship_state_file)
    mkdir -p "$(dirname "$sf")"
    local tmp="${sf}.tmp.$$"
    {
        [[ -f "$sf" ]] && grep -v "^${key}=" "$sf" 2>/dev/null || true
        printf '%s=%s\n' "$key" "$val"
    } > "$tmp"
    mv "$tmp" "$sf"
}

freship_read_state() {
    local key=$1 default=${2:-}
    local sf line
    sf=$(freship_state_file)
    [[ ! -f "$sf" ]] && { echo "$default"; return; }
    line=$(grep "^${key}=" "$sf" 2>/dev/null | tail -n 1)
    [[ -z "$line" ]] && { echo "$default"; return; }
    echo "${line#${key}=}"
}

freship_clear_state() {
    rm -f "$(freship_state_file)"
}

get_random_coord() {
    local base=$1 range=$2
    local offset
    offset=$(awk "BEGIN {print ( ( ($RANDOM % ($range * 2)) - $range ) / 10000 )}")
    awk "BEGIN {print ($base + $offset)}"
}

node_hash_from_ip() {
    echo -n "${1:-127.0.0.1}" | cksum | awk '{print $1}'
}

persona_from_ip() {
    local ip=$1 ua_file=$2
    PERSONA_UA=""
    if [[ ! -f "$ua_file" ]]; then
        PERSONA_UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
        return
    fi
    mapfile -t _ua_pool < <(grep -v '^[[:space:]]*$' "$ua_file")
    local total=${#_ua_pool[@]}
    if [[ "$total" -eq 0 ]]; then
        PERSONA_UA="Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
        return
    fi
    local seed idx1 idx2 idx3
    seed=$(node_hash_from_ip "$ip")
    idx1=$(( seed % total ))
    idx2=$(( (seed * 17) % total ))
    idx3=$(( (seed * 31) % total ))
    local pick=$(( RANDOM % 3 ))
    case $pick in
        0) PERSONA_UA="${_ua_pool[$idx1]}" ;;
        1) PERSONA_UA="${_ua_pool[$idx2]}" ;;
        *) PERSONA_UA="${_ua_pool[$idx3]}" ;;
    esac
}

ua_platform_from_string() {
    local ua=$1
    if [[ "$ua" == *"Android"* ]]; then echo "android"
    elif [[ "$ua" == *"iPhone"* || "$ua" == *"iPad"* ]]; then echo "ios"
    elif [[ "$ua" == *"Macintosh"* ]]; then echo "macos"
    elif [[ "$ua" == *"Linux"* ]]; then echo "linux"
    else echo "windows"
    fi
}

resolve_browse_curl() {
    BROWSE_CURL="curl"
    BROWSE_TLS_MODE="Native"
    local candidate
    for candidate in curl_chrome124 curl_chrome116 curl_chrome110; do
        if [[ -x "${INSTALL_DIR}/bin/${candidate}" ]]; then
            BROWSE_CURL="${INSTALL_DIR}/bin/${candidate}"
            BROWSE_TLS_MODE="${candidate}"
            return
        fi
    done
}

build_bind_args() {
    CURL_BIND_ARGS=()
    DYNAMIC_IP_PREF="-4"
    [[ -z "$BIND_IP" ]] && return
    local raw
    raw=$(echo "$BIND_IP" | tr -d '[]')
    if ! ip addr show 2>/dev/null | grep -Fq "$raw"; then
        freship_log "LIB" "WARN" "出口 IP (${raw}) 不在本机网卡，降级为默认路由"
        return
    fi
    CURL_BIND_ARGS=(--interface "$BIND_IP")
    if [[ "$BIND_IP" == *":"* ]]; then
        DYNAMIC_IP_PREF="-6"
    elif [[ "$BIND_IP" == *"."* ]]; then
        DYNAMIC_IP_PREF="-4"
    fi
}

cookie_file_for() {
    local prefix=$1
    local hash
    hash=$(node_hash_from_ip "${BIND_IP:-127.0.0.1}")
    echo "${INSTALL_DIR}/data/cookies/${prefix}_${hash}.txt"
}

acquire_cookie_lock() {
    local cookie_file=$1
    COOKIE_LOCK_FILE="${cookie_file}.lock"
    exec {COOKIE_LOCK_FD}>"$COOKIE_LOCK_FILE"
    if ! flock -n "$COOKIE_LOCK_FD"; then
        return 1
    fi
    find "${INSTALL_DIR}/data/cookies" -type f -name "*.txt" -mtime +14 -delete 2>/dev/null || true
    return 0
}

release_cookie_lock() {
    [[ -n "${COOKIE_LOCK_FD:-}" ]] && flock -u "$COOKIE_LOCK_FD" 2>/dev/null || true
}

encode_uri_component() {
    local raw=$1
    if command -v jq >/dev/null 2>&1; then
        printf '%s' "$raw" | jq -sRr @uri 2>/dev/null && return
    fi
    printf '%s' "$raw" | sed 's/ /+/g'
}

map_curl_exit() {
    case $1 in
        6)  echo "ERR_DNS" ;;
        7)  echo "ERR_CONN" ;;
        28) echo "ERR_TIMEOUT" ;;
        35) echo "ERR_TLS" ;;
        56) echo "ERR_RESET" ;;
        *)  echo "ERR_${1}" ;;
    esac
}

region_json_path() {
    if [[ -n "${REGION_PATH:-}" && -f "${INSTALL_DIR}/data/regions/${REGION_PATH}.json" ]]; then
        echo "${INSTALL_DIR}/data/regions/${REGION_PATH}.json"
        return
    fi
    find "${INSTALL_DIR}/data/regions" -name "*.json" 2>/dev/null | head -n 1
}

lang_hl_from_params() {
    local params=${1:-}
    if [[ "$params" =~ hl=([^&]+) ]]; then
        echo "${BASH_REMATCH[1]}"
    else
        echo "en"
    fi
}

freship_should_skip_quiet_hours() {
    local local_hour
    local_hour=$(date -u -d "${UTC_OFFSET:-+0} hours" +%H 2>/dev/null || date +%H)
    [[ "$local_hour" -ge 1 && "$local_hour" -le 6 ]]
}

freship_should_skip_low_activity() {
    local daily_seed activity
    daily_seed=$(echo "$(date +%Y%m%d)" | cksum | awk '{print $1}')
    activity=$(( daily_seed % 100 ))
    [[ "$activity" -lt 30 && $(( RANDOM % 100 )) -gt "$activity" ]]
}

freship_should_skip_daily_maintain() {
    [[ "${CI:-}" == "true" ]] && return 1
    local mode last_probe today
    mode=$(freship_read_state "RUN_MODE" "")
    [[ "$mode" != "maintain" ]] && return 1
    last_probe=$(freship_read_state "LAST_PROBE_UTC" "")
    [[ -z "$last_probe" ]] && return 1
    today=$(date -u '+%Y-%m-%d')
    [[ "$last_probe" == "${today}"* ]]
}

poisson_sleep_seconds() {
    local dice=$(( RANDOM % 100 ))
    if [[ "$dice" -lt 45 ]]; then
        echo $(( 8 + RANDOM % 13 ))
    elif [[ "$dice" -lt 80 ]]; then
        echo $(( 20 + RANDOM % 41 ))
    elif [[ "$dice" -lt 95 ]]; then
        echo $(( 60 + RANDOM % 121 ))
    else
        echo $(( 180 + RANDOM % 300 ))
    fi
}

parse_jump_gl() {
    local jump_loc=$1
    local jump_gl=""
    if [[ -z "$jump_loc" ]]; then
        jump_gl="US"
    elif [[ "$jump_loc" == *".google.cn"* || "$jump_loc" == *"gl=CN"* ]]; then
        jump_gl="CN"
    elif [[ "$jump_loc" == *"gl="* ]]; then
        jump_gl=$(echo "$jump_loc" | grep -o 'gl=[A-Za-z]\{2\}' | head -n 1 | cut -d'=' -f2 | tr 'a-z' 'A-Z')
    else
        local jump_domain last_ext
        jump_domain=$(echo "$jump_loc" | grep -o 'google\.[a-z\.]*' | head -n 1 | sed 's/google\.//')
        case "$jump_domain" in
            com) jump_gl="US" ;;
            com.hk) jump_gl="HK" ;;
            com.tw) jump_gl="TW" ;;
            co.jp) jump_gl="JP" ;;
            co.uk) jump_gl="GB" ;;
            co.kr) jump_gl="KR" ;;
            co.in) jump_gl="IN" ;;
            co.id) jump_gl="ID" ;;
            co.th) jump_gl="TH" ;;
            com.sg) jump_gl="SG" ;;
            com.my) jump_gl="MY" ;;
            com.au) jump_gl="AU" ;;
            com.br) jump_gl="BR" ;;
            com.mx) jump_gl="MX" ;;
            com.ar) jump_gl="AR" ;;
            co.za) jump_gl="ZA" ;;
            cn) jump_gl="CN" ;;
            "") jump_gl="" ;;
            *)
                last_ext=$(echo "$jump_domain" | awk -F'.' '{print $NF}' | tr 'a-z' 'A-Z')
                if [[ ${#last_ext} -eq 2 ]]; then
                    jump_gl="$last_ext"
                else
                    jump_gl="US"
                fi
                ;;
        esac
    fi
    echo "$jump_gl"
}

# 三核探针国家级判定；设置 PROBE_SCORE / PROBE_MSG，返回 0=ok 1=drift 2=cn 3=fail
evaluate_probe_score() {
    local jump_gl=$1 yt_pr_gl=$2 yt_mu_gl=$3
    local target_cc="${TARGET_CC:-${REGION_CODE%%-*}}"
    [[ "$target_cc" == "UK" ]] && target_cc="GB"

    local valid_probes=0 is_cn=0 val
    for val in "$jump_gl" "$yt_pr_gl" "$yt_mu_gl"; do
        [[ -n "$val" ]] && valid_probes=$((valid_probes + 1))
        [[ "$val" == "CN" ]] && is_cn=1
    done

    if [[ "$valid_probes" -eq 0 ]]; then
        PROBE_SCORE="fail"
        PROBE_MSG="探针失效 (三核均无有效回波)"
        return 3
    fi
    if [[ "$is_cn" -eq 1 ]]; then
        PROBE_SCORE="cn"
        PROBE_MSG="送中 (Jump: ${jump_gl:-无} | Prem: ${yt_pr_gl:-无} | Music: ${yt_mu_gl:-无})"
        return 2
    fi

    local yt_match=0
    [[ "$yt_pr_gl" == "$target_cc" ]] && yt_match=1
    [[ "$yt_mu_gl" == "$target_cc" ]] && yt_match=1

    if [[ "$yt_match" -eq 1 ]]; then
        PROBE_SCORE="ok"
        if [[ -n "$jump_gl" && "$jump_gl" != "$target_cc" ]]; then
            PROBE_MSG="区域达标 (YT 匹配, Jump 漂移至 ${jump_gl}) | Prem: ${yt_pr_gl:-无} | Music: ${yt_mu_gl:-无}"
        else
            PROBE_MSG="区域达标 | Jump: ${jump_gl:-无} | Prem: ${yt_pr_gl:-无} | Music: ${yt_mu_gl:-无}"
        fi
        return 0
    fi
    if [[ "$jump_gl" == "$target_cc" ]]; then
        PROBE_SCORE="ok"
        PROBE_MSG="区域达标 (Jump 匹配) | Jump: ${jump_gl} | Prem: ${yt_pr_gl:-无} | Music: ${yt_mu_gl:-无}"
        return 0
    fi

    PROBE_SCORE="drift"
    PROBE_MSG="区域漂移 (目标 ${target_cc}) | Jump: ${jump_gl:-无} | Prem: ${yt_pr_gl:-无} | Music: ${yt_mu_gl:-无}"
    return 1
}
