#!/bin/bash
# ==============================================================================
# IP-Sentinel 单机精简版 v2.0
# 改进点（相较 v1.0）：
#   1. TLS 指纹伪装：优先使用 curl-impersonate（模拟真实 Chrome JA3 握手）
#   2. Referer 链：动作之间传递来源 URL，模拟真实页面跳转
#   3. generate_204 逻辑修正：仅 Android UA 执行，桌面端改为 Google 图片
#   4. Trust 模块分层权重：新闻 URL 70% 高频，政府/银行 URL 30% 低频
#   5. Google 动作类型加权分布：搜索60% News25% Maps10% 探针5%
#   6. 每日活跃度随机化：以日期为种子，模拟"忙碌天"和"闲散天"
# 依赖：curl（必须）、jq、cksum
#       curl-impersonate（强烈推荐，install.sh 会自动安装）
# ==============================================================================

INSTALL_DIR="/opt/ip_sentinel"
CONFIG_FILE="${INSTALL_DIR}/config.conf"
LOG_FILE="${INSTALL_DIR}/logs/sentinel.log"
LOCK_FILE="/tmp/ip_sentinel.lock"

# ------------------------------------------------------------------------------
# 0. 加载配置
# ------------------------------------------------------------------------------
if [ ! -f "$CONFIG_FILE" ]; then
    echo "❌ 配置文件不存在: $CONFIG_FILE"
    exit 1
fi
source "$CONFIG_FILE"

# ------------------------------------------------------------------------------
# 1. 排他锁：防止并发重入
# ------------------------------------------------------------------------------
exec 200>"$LOCK_FILE"
if ! flock -n 200; then
    log "SYSTEM" "WARN " "上一轮任务尚未结束，本次取消"
    exit 0
fi

# ------------------------------------------------------------------------------
# 2. 日志函数
# ------------------------------------------------------------------------------
log() {
    local MODULE="$1" LEVEL="$2" MSG="$3"
    mkdir -p "${INSTALL_DIR}/logs"
    printf "[%s UTC] [%-7s] [%-5s] [%s] %s\n" \
        "$(date -u '+%Y-%m-%d %H:%M:%S')" \
        "$MODULE" "$LEVEL" "${REGION_CODE:-??}" "$MSG" \
        >> "$LOG_FILE"
}

# ------------------------------------------------------------------------------
# 3. TLS 指纹层：优先使用 curl-impersonate，否则降级到普通 curl
#
#    背景：普通 curl 的 TLS 握手特征（JA3 指纹）与浏览器完全不同，
#    即使 User-Agent 伪装成 Chrome，握手包仍会暴露真实身份。
#    curl-impersonate 在 TLS 握手层面完整复制了 Chrome 的行为。
#
#    CURL_CMD  = 实际使用的 curl 可执行路径
#    IS_IMPERSONATED = true 时才有 TLS 伪装效果
# ------------------------------------------------------------------------------
CURL_CMD=""
IS_IMPERSONATED=false

# 按优先级检测可用的 curl-impersonate 版本
for candidate in \
    "${INSTALL_DIR}/bin/curl_chrome124" \
    "${INSTALL_DIR}/bin/curl_chrome116" \
    "${INSTALL_DIR}/bin/curl_chrome110" \
    "/usr/local/bin/curl_chrome124" \
    "/usr/local/bin/curl_chrome116"; do
    if [ -x "$candidate" ]; then
        CURL_CMD="$candidate"
        IS_IMPERSONATED=true
        break
    fi
done

# 兜底：使用系统 curl（TLS 指纹不完美，但其他逻辑仍然有效）
if [ -z "$CURL_CMD" ]; then
    CURL_CMD="curl"
    IS_IMPERSONATED=false
    log "SYSTEM" "WARN " "未找到 curl-impersonate，使用系统 curl（TLS 指纹未伪装）"
fi

# ------------------------------------------------------------------------------
# 4. 深夜静默：本地时间 0~6 点不执行
# ------------------------------------------------------------------------------
CURRENT_HOUR=$(date +%H)
if [ "$CURRENT_HOUR" -ge 0 ] && [ "$CURRENT_HOUR" -lt 6 ]; then
    log "SYSTEM" "INFO " "深夜静默时段 (${CURRENT_HOUR}:xx)，退出"
    exit 0
fi

# ------------------------------------------------------------------------------
# 5. 每日活跃度随机化（改进点6）
#
#    以"今天的日期"为种子生成0~99的活跃度值，同一天内无论触发多少次
#    结果都相同（保证一天内行为一致，不会上午跳过下午又突然活跃）
#
#    低活跃天（活跃度<30，约占30%的天数）：降低50%的执行概率
#    模拟真实用户有时候整天很忙、不怎么上网的情况
# ------------------------------------------------------------------------------
DAILY_SEED=$(date '+%Y%m%d' | cksum | awk '{print $1}')
DAILY_ACTIVE=$(( DAILY_SEED % 100 ))

if [ "$DAILY_ACTIVE" -lt 30 ] && [ $(( RANDOM % 2 )) -eq 0 ]; then
    log "SYSTEM" "INFO " "今日低活跃模式（活跃度=${DAILY_ACTIVE}），本次跳过"
    exit 0
fi

# ------------------------------------------------------------------------------
# 6. 防并发抖动（Cron Jitter）
#    非终端环境下随机等待 0~180 秒，避免全球同机房同时发起请求
# ------------------------------------------------------------------------------
if [ ! -t 1 ]; then
    JITTER=$((RANDOM % 180))
    log "SYSTEM" "INFO " "防并发随机休眠 ${JITTER}s..."
    sleep "$JITTER"
fi

# ------------------------------------------------------------------------------
# 7. 检查必要数据文件
# ------------------------------------------------------------------------------
UA_FILE="${INSTALL_DIR}/data/user_agents.txt"
KW_FILE="${INSTALL_DIR}/data/keywords/kw_${REGION_CODE}.txt"
REGION_JSON=$(find "${INSTALL_DIR}/data/regions" -name "*.json" 2>/dev/null | head -n 1)

if [ ! -f "$UA_FILE" ] || [ ! -f "$KW_FILE" ]; then
    log "SYSTEM" "ERROR" "缺少 UA 库或关键词库，退出"
    exit 1
fi
if [ -z "$REGION_JSON" ] || [ ! -f "$REGION_JSON" ]; then
    log "SYSTEM" "ERROR" "缺少区域规则 JSON，退出"
    exit 1
fi

# ------------------------------------------------------------------------------
# 8. 从区域 JSON 读取配置
# ------------------------------------------------------------------------------
BASE_LAT=$(jq -r   '.google_module.base_lat'   "$REGION_JSON")
BASE_LON=$(jq -r   '.google_module.base_lon'   "$REGION_JSON")
LANG_PARAMS=$(jq -r '.google_module.lang_params' "$REGION_JSON")
REGION_NAME=$(jq -r '.region_name'              "$REGION_JSON")

# Trust 模块分层读取（改进点4）
# static_urls：政府/银行/高校等高权重骨干站，变化少
# white_urls ：混合了 static + 每日新闻 RSS 链接，每天由 GitHub Actions 刷新
mapfile -t STATIC_URLS < <(jq -r '.trust_module.static_urls[]' "$REGION_JSON" 2>/dev/null)
mapfile -t WHITE_URLS  < <(jq -r '.trust_module.white_urls[]'  "$REGION_JSON" 2>/dev/null)

# 兜底
if [ "${#WHITE_URLS[@]}" -eq 0 ] && [ "${#STATIC_URLS[@]}" -eq 0 ]; then
    WHITE_URLS=("https://en.wikipedia.org/wiki/Special:Random"
                "https://www.apple.com/" "https://www.microsoft.com/")
fi

# 关键词和 UA 读入数组
mapfile -t KEYWORDS < <(grep -v '^$' "$KW_FILE")
mapfile -t UA_POOL  < <(grep -v '^$' "$UA_FILE")

# ------------------------------------------------------------------------------
# 9. 自动检测/绑定公网 IP
# ------------------------------------------------------------------------------
if [ -z "$BIND_IP" ]; then
    BIND_IP=$(curl -4 -s -m 5 api.ip.sb/ip 2>/dev/null \
              || curl -4 -s -m 5 ifconfig.me 2>/dev/null)
    BIND_IP=$(echo "$BIND_IP" | tr -d '[:space:]')
fi

[[ "$BIND_IP" == *":"* ]] && [[ "$BIND_IP" != "["* ]] && BIND_ADDR="[${BIND_IP}]" || BIND_ADDR="$BIND_IP"
[[ "$BIND_IP" == *":"* ]] && IP_FLAG="-6" || IP_FLAG="-4"

RAW_IP=$(echo "$BIND_ADDR" | tr -d '[]')
CURL_BIND=""
if ip addr show 2>/dev/null | grep -qw "$RAW_IP"; then
    CURL_BIND="--interface $BIND_ADDR"
else
    log "SYSTEM" "WARN " "IP ($RAW_IP) 不在本机网卡，使用默认路由"
fi

# ------------------------------------------------------------------------------
# 10. 哈希锚定指纹（Hash-Seeded Persona）
#
#     同一台机器永远只"扮演"3台固定设备，而不是每次随机换UA。
#     逻辑：用本机IP做CRC32，映射到UA池的3个固定下标。
#     这样对外呈现的设备一致性，与真实用户长期使用固定几台设备的行为一致。
# ------------------------------------------------------------------------------
TOTAL_UA="${#UA_POOL[@]}"
if [ "$TOTAL_UA" -gt 0 ]; then
    SEED=$(echo -n "$BIND_IP" | cksum | awk '{print $1}')
    IDX1=$(( SEED          % TOTAL_UA ))
    IDX2=$(( (SEED * 17)   % TOTAL_UA ))
    IDX3=$(( (SEED * 31)   % TOTAL_UA ))
    MY_UA_POOL=("${UA_POOL[$IDX1]}" "${UA_POOL[$IDX2]}" "${UA_POOL[$IDX3]}")
else
    MY_UA_POOL=("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
fi
SESSION_UA="${MY_UA_POOL[$RANDOM % ${#MY_UA_POOL[@]}]}"

# 判断本次会话是移动端还是桌面端（影响动作4的行为选择）
IS_MOBILE=false
[[ "$SESSION_UA" == *"Android"* ]] || [[ "$SESSION_UA" == *"iPhone"* ]] && IS_MOBILE=true

# ------------------------------------------------------------------------------
# 11. 会话级 GPS 坐标（在城市中心 ~3km 范围内随机漂移）
# ------------------------------------------------------------------------------
get_coord() {
    local BASE="$1" RANGE="$2"
    awk -v b="$BASE" -v r="$RANGE" -v s="$RANDOM" \
        'BEGIN { srand(s); printf "%.6f", b + ((rand()*2*r - r)/10000) }'
}
SESSION_LAT=$(get_coord "$BASE_LAT" 270)
SESSION_LON=$(get_coord "$BASE_LON" 270)

log "SYSTEM" "INFO " "===== 开始养护会话 ====="
log "SYSTEM" "INFO " "区域: ${REGION_NAME} | IP: ${BIND_IP} | 活跃度: ${DAILY_ACTIVE}"
log "SYSTEM" "INFO " "TLS伪装: $($IS_IMPERSONATED && echo '✅ curl-impersonate' || echo '⚠️ 普通curl')"
log "SYSTEM" "INFO " "设备: ${SESSION_UA:0:60}..."
log "SYSTEM" "INFO " "驻留坐标: ${SESSION_LAT}, ${SESSION_LON}"

# ==============================================================================
# 模块选择：两个模块均开启时，70% 跑 Google，30% 跑 Trust
# ==============================================================================
RUN_GOOGLE=false
RUN_TRUST=false

if [ "${ENABLE_GOOGLE:-true}" = "true" ] && [ "${ENABLE_TRUST:-true}" = "true" ]; then
    [ $(( RANDOM % 100 )) -lt 70 ] && RUN_GOOGLE=true || RUN_TRUST=true
elif [ "${ENABLE_GOOGLE:-true}" = "true" ]; then RUN_GOOGLE=true
elif [ "${ENABLE_TRUST:-true}"  = "true" ]; then RUN_TRUST=true
else
    log "SYSTEM" "WARN " "两个模块均未启用，退出"
    exit 0
fi

# ==============================================================================
# ██  模块 A：Google 区域纠偏
#
# 改进点汇总：
#   - 改进2：动作间传递 Referer，模拟真实页面跳转链
#   - 改进3：generate_204 仅限 Android UA，桌面端改为 Google 图片
#   - 改进5：动作类型按真实用户分布加权（搜索60% News25% Maps10% 探针5%）
# ==============================================================================
if $RUN_GOOGLE; then
    log "Google" "START" "---------- Google 定位纠偏模块启动 ----------"

    TOTAL_ACTIONS=$(( 6 + RANDOM % 5 ))
    log "Google" "INFO " "计划执行 ${TOTAL_ACTIONS} 个动作"

    PREV_URL=""   # 改进2：上一个请求的 URL，用作下一个请求的 Referer

    for (( i=1; i<=TOTAL_ACTIONS; i++ )); do

        # 动作级 GPS 微抖动（~10 米范围，模拟手持设备移动）
        ACT_LAT=$(get_coord "$SESSION_LAT" 1)
        ACT_LON=$(get_coord "$SESSION_LON" 1)

        # 从热搜词库随机抽取
        KEYWORD="${KEYWORDS[$RANDOM % ${#KEYWORDS[@]}]}"
        ENC_KW=$(printf '%s' "$KEYWORD" | jq -sRr @uri)

        # 改进2：构建 Referer 参数（第一个请求无 Referer，模拟直接打开浏览器）
        REFERER_OPT=""
        [ -n "$PREV_URL" ] && REFERER_OPT="-e $PREV_URL"

        # 改进5：加权动作类型分布
        #   真实用户行为：搜索>>浏览新闻>地图查询>系统探针
        ROLL=$(( RANDOM % 100 ))
        if   [ "$ROLL" -lt 60 ]; then ACTION_TYPE=1   # 搜索   60%
        elif [ "$ROLL" -lt 85 ]; then ACTION_TYPE=2   # News   25%
        elif [ "$ROLL" -lt 95 ]; then ACTION_TYPE=3   # Maps   10%
        else                          ACTION_TYPE=4   # 探针    5%
        fi

        # 组装公共请求头（Accept-Language 从 lang_params 提取语言代码）
        LANG_CODE=$(echo "$LANG_PARAMS" | grep -o 'hl=[^&]*' | cut -d= -f2)
        COMMON_HEADERS=(
            -A "$SESSION_UA"
            -H "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8"
            -H "Accept-Language: ${LANG_CODE},en;q=0.5"
            -H "Accept-Encoding: gzip, deflate, br"
            -H "Connection: keep-alive"
            -H "Upgrade-Insecure-Requests: 1"
            -H "Sec-Fetch-Dest: document"
            -H "Sec-Fetch-Mode: navigate"
            -H "Sec-Fetch-Site: $( [ -n "$PREV_URL" ] && echo 'same-origin' || echo 'none' )"
        )

        case $ACTION_TYPE in
            1)  # 行为：Google 关键词搜索
                TARGET_URL="https://www.google.com/search?q=${ENC_KW}&${LANG_PARAMS}"
                CODE=$($CURL_CMD $CURL_BIND $IP_FLAG \
                    -m 20 -s -L -o /dev/null -w "%{http_code}" \
                    "${COMMON_HEADERS[@]}" $REFERER_OPT \
                    "$TARGET_URL")
                ACTION_DESC="搜索 [${KEYWORD}]"
                ;;

            2)  # 行为：Google News 本地新闻首页
                TARGET_URL="https://news.google.com/home?${LANG_PARAMS}"
                CODE=$($CURL_CMD $CURL_BIND $IP_FLAG \
                    -m 20 -s -L -o /dev/null -w "%{http_code}" \
                    "${COMMON_HEADERS[@]}" $REFERER_OPT \
                    "$TARGET_URL")
                ACTION_DESC="Google News 首页"
                ;;

            3)  # 行为：Google Maps 坐标查询（携带本次抖动坐标）
                TARGET_URL="https://www.google.com/maps/search/${ENC_KW}/@${ACT_LAT},${ACT_LON},17z?${LANG_PARAMS}"
                CODE=$($CURL_CMD $CURL_BIND $IP_FLAG \
                    -m 20 -s -o /dev/null -w "%{http_code}" \
                    "${COMMON_HEADERS[@]}" $REFERER_OPT \
                    "$TARGET_URL")
                ACTION_DESC="Maps 查询 [@${ACT_LAT},${ACT_LON}]"
                ;;

            4)  # 改进3：根据设备类型选择不同行为
                #   Android UA → generate_204（Android 系统网络探针，只有手机才发）
                #   桌面端 UA  → Google 图片搜索（桌面用户的常见行为）
                if $IS_MOBILE; then
                    TARGET_URL="https://connectivitycheck.gstatic.com/generate_204"
                    ACTION_DESC="Android 网络探针"
                else
                    TARGET_URL="https://www.google.com/imghp?${LANG_PARAMS}"
                    ACTION_DESC="Google 图片首页"
                fi
                CODE=$($CURL_CMD $CURL_BIND $IP_FLAG \
                    -m 10 -s -o /dev/null -w "%{http_code}" \
                    "${COMMON_HEADERS[@]}" $REFERER_OPT \
                    "$TARGET_URL")
                ;;
        esac

        log "Google" "EXEC " "动作[${i}/${TOTAL_ACTIONS}] HTTP=${CODE} | ${ACTION_DESC}"

        # 改进2：更新 Referer 链（保存本次请求的 URL 供下次使用）
        PREV_URL="$TARGET_URL"

        # 动作间停留：90~120 秒（模拟真人阅读停顿）
        if [ "$i" -lt "$TOTAL_ACTIONS" ]; then
            WAIT=$(( 90 + RANDOM % 31 ))
            log "Google" "WAIT " "模拟阅读停留 ${WAIT}s..."
            sleep "$WAIT"
        fi
    done

    # --------------------------------------------------------------------------
    # 三核自检探针：交叉验证 IP 当前被 Google 判定在哪个国家
    # --------------------------------------------------------------------------
    log "Google" "INFO " "启动三核交叉验证..."

    # 探针1：Google 跳转域名
    JUMP_HDR=$($CURL_CMD $CURL_BIND $IP_FLAG -m 10 -sI "http://www.google.com/")
    JUMP_LOC=$(echo "$JUMP_HDR" | grep -i "^location:" | tr -d '\r\n')
    JUMP_GL=""
    if [ -z "$JUMP_LOC" ]; then
        JUMP_GL="US"
    elif [[ "$JUMP_LOC" == *".google.cn"* ]] || [[ "$JUMP_LOC" == *"gl=CN"* ]]; then
        JUMP_GL="CN"
    elif [[ "$JUMP_LOC" == *"gl="* ]]; then
        JUMP_GL=$(echo "$JUMP_LOC" | grep -o 'gl=[A-Za-z]\{2\}' | head -1 | cut -d= -f2 | tr 'a-z' 'A-Z')
    else
        JUMP_DOM=$(echo "$JUMP_LOC" | grep -o 'google\.[a-z\.]*' | head -1 | sed 's/google\.//')
        case "$JUMP_DOM" in
            "com")    JUMP_GL="US" ;;  "cn")      JUMP_GL="CN" ;;
            "co.jp")  JUMP_GL="JP" ;;  "co.uk")   JUMP_GL="GB" ;;
            "com.hk") JUMP_GL="HK" ;;  "com.tw")  JUMP_GL="TW" ;;
            "co.kr")  JUMP_GL="KR" ;;  "com.sg")  JUMP_GL="SG" ;;
            "com.au") JUMP_GL="AU" ;;  "com.my")  JUMP_GL="MY" ;;
            "de")     JUMP_GL="DE" ;;  "fr")      JUMP_GL="FR" ;;
            "nl")     JUMP_GL="NL" ;;  "es")      JUMP_GL="ES" ;;
            *) LAST=$(echo "$JUMP_DOM" | awk -F'.' '{print $NF}' | tr 'a-z' 'A-Z')
               [ "${#LAST}" -eq 2 ] && JUMP_GL="$LAST" || JUMP_GL="US" ;;
        esac
    fi

    # 探针2：YouTube Premium contentRegion
    YT_PR_HTML=$($CURL_CMD $CURL_BIND $IP_FLAG -m 15 -s -L \
        -A "$SESSION_UA" "https://www.youtube.com/premium")
    if echo "$YT_PR_HTML" | grep -q 'www.google.cn'; then
        YT_PR_GL="CN"
    else
        YT_PR_GL=$(echo "$YT_PR_HTML" | grep -o '"contentRegion":"[A-Za-z]\{2\}"'        | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
        [ -z "$YT_PR_GL" ] && \
        YT_PR_GL=$(echo "$YT_PR_HTML" | grep -o '"countryCode":"[A-Za-z]\{2\}"'          | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
        [ -z "$YT_PR_GL" ] && \
        YT_PR_GL=$(echo "$YT_PR_HTML" | grep -o '"INNERTUBE_CONTEXT_GL":"[A-Za-z]\{2\}"' | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
    fi

    # 探针3：YouTube Music INNERTUBE_CONTEXT_GL
    YT_MU_HTML=$($CURL_CMD $CURL_BIND $IP_FLAG -m 15 -s -L \
        -A "$SESSION_UA" "https://music.youtube.com/")
    if echo "$YT_MU_HTML" | grep -q 'www.google.cn'; then
        YT_MU_GL="CN"
    else
        YT_MU_GL=$(echo "$YT_MU_HTML" | grep -o '"INNERTUBE_CONTEXT_GL":"[A-Za-z]\{2\}"' | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
        [ -z "$YT_MU_GL" ] && \
        YT_MU_GL=$(echo "$YT_MU_HTML" | grep -o '"countryCode":"[A-Za-z]\{2\}"'          | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
        [ -z "$YT_MU_GL" ] && \
        YT_MU_GL=$(echo "$YT_MU_HTML" | grep -o '"GL":"[A-Za-z]\{2\}"'                   | head -1 | cut -d'"' -f4 | tr 'a-z' 'A-Z')
    fi

    # 三核判定
    TARGET_CC="${REGION_CODE%%-*}"
    [ "$TARGET_CC" = "UK" ] && TARGET_CC="GB"
    IS_CN=0; VALID=0
    for V in "$JUMP_GL" "$YT_PR_GL" "$YT_MU_GL"; do
        [ -n "$V" ] && (( VALID++ ))
        [ "$V" = "CN" ] && IS_CN=1
    done

    if   [ "$VALID" -eq 0 ]; then
        VERDICT="🚨 三核探针全部失效（可能被严重风控拦截）"
    elif [ "$IS_CN" -eq 1 ]; then
        VERDICT="❌ 高危！IP 已被 Google 判定为中国大陆"
    else
        YT_OK=0
        [ "$YT_PR_GL" = "$TARGET_CC" ] && YT_OK=1
        [ "$YT_MU_GL" = "$TARGET_CC" ] && YT_OK=1
        if [ "$YT_OK" -eq 1 ]; then
            if [ -n "$JUMP_GL" ] && [ "$JUMP_GL" != "$TARGET_CC" ]; then
                VERDICT="✅ 目标区域达成（YT核心成功，跳转副探针漂移至 ${JUMP_GL}）"
            else
                VERDICT="✅ 完美达成 | Jump:${JUMP_GL:-无} Prem:${YT_PR_GL:-无} Music:${YT_MU_GL:-无}"
            fi
        else
            VERDICT="⚠️ 漂移！目标=${TARGET_CC} | Jump:${JUMP_GL:-无} Prem:${YT_PR_GL:-无} Music:${YT_MU_GL:-无}"
        fi
    fi

    log "Google" "SCORE" "$VERDICT"
    log "Google" "END  " "---------- Google 模块结束 ----------"

    # 可选 Telegram 推送
    if [ -n "$TG_TOKEN" ] && [ -n "$CHAT_ID" ]; then
        curl -s -X POST "https://api.telegram.org/bot${TG_TOKEN}/sendMessage" \
            -d "chat_id=${CHAT_ID}" \
            -d "text=🛡 IP-Sentinel | ${REGION_NAME}
${VERDICT}" >/dev/null 2>&1
    fi
fi

# ==============================================================================
# ██  模块 B：IP 信用净化
#
# 改进点汇总：
#   - 改进4：分层权重访问
#     70% 概率从 white_urls（含当日新闻）中选取 → 模拟普通人看新闻
#     30% 概率从 static_urls（政府/银行/高校）中选取 → 偶尔访问权威站
#   - 改进2：同样传递 Referer 链
# ==============================================================================
if $RUN_TRUST; then
    log "Trust " "START" "---------- IP 信用净化模块启动 ----------"
    log "Trust " "INFO " "white_urls: ${#WHITE_URLS[@]} 条 | static_urls: ${#STATIC_URLS[@]} 条"

    STEP_COUNT=$(( 3 + RANDOM % 4 ))
    SUCCESS=0
    PREV_URL=""

    for (( i=1; i<=STEP_COUNT; i++ )); do

        # 改进4：分层权重选取目标 URL
        ROLL=$(( RANDOM % 10 ))
        if [ "$ROLL" -lt 7 ] && [ "${#WHITE_URLS[@]}" -gt 0 ]; then
            # 70%：新闻/动态 URL（每日更新的 RSS 链接，更像真实用户行为）
            TARGET="${WHITE_URLS[$RANDOM % ${#WHITE_URLS[@]}]}"
            URL_TYPE="新闻"
        elif [ "${#STATIC_URLS[@]}" -gt 0 ]; then
            # 30%：骨干站（政府/银行，高权重但低频访问）
            TARGET="${STATIC_URLS[$RANDOM % ${#STATIC_URLS[@]}]}"
            URL_TYPE="权威站"
        else
            TARGET="${WHITE_URLS[$RANDOM % ${#WHITE_URLS[@]}]}"
            URL_TYPE="备用"
        fi

        # 改进2：Referer 链
        REFERER_OPT=""
        [ -n "$PREV_URL" ] && REFERER_OPT="-e $PREV_URL"

        HTTP_CODE=$($CURL_CMD $CURL_BIND $IP_FLAG \
            -A "$SESSION_UA" \
            -H "Accept: text/html,application/xhtml+xml;q=0.9,image/avif,image/webp,*/*;q=0.8" \
            -H "Accept-Language: $(echo "$LANG_PARAMS" | grep -o 'hl=[^&]*' | cut -d= -f2),en;q=0.5" \
            -H "Sec-Fetch-Dest: document" \
            -H "Sec-Fetch-Mode: navigate" \
            -H "Sec-Fetch-Site: $( [ -n "$PREV_URL" ] && echo 'same-origin' || echo 'none' )" \
            -H "Upgrade-Insecure-Requests: 1" \
            --compressed \
            $REFERER_OPT \
            -s -o /dev/null -w "%{http_code}" -m 20 "$TARGET")

        if [[ "$HTTP_CODE" =~ ^(20[0-9]|30[0-8])$ ]]; then
            log "Trust " "EXEC " "动作[${i}/${STEP_COUNT}] HTTP=${HTTP_CODE} ✓ [${URL_TYPE}] ${TARGET}"
            (( SUCCESS++ ))
        else
            log "Trust " "EXEC " "动作[${i}/${STEP_COUNT}] HTTP=${HTTP_CODE} ✗ [${URL_TYPE}] ${TARGET}"
        fi

        PREV_URL="$TARGET"

        if [ "$i" -lt "$STEP_COUNT" ]; then
            WAIT=$(( 45 + RANDOM % 76 ))
            log "Trust " "WAIT " "模拟停留 ${WAIT}s..."
            sleep "$WAIT"
        fi
    done

    if [ "$SUCCESS" -ge $(( STEP_COUNT / 2 )) ]; then
        log "Trust " "SCORE" "✅ 净化完成（${SUCCESS}/${STEP_COUNT} 条无害流量注入成功）"
    else
        log "Trust " "SCORE" "⚠️ 净化受阻（${SUCCESS}/${STEP_COUNT} 成功）"
    fi
    log "Trust " "END  " "---------- 信用净化模块结束 ----------"
fi

# ==============================================================================
# 日志裁剪
# ==============================================================================
if [ -f "$LOG_FILE" ] && [ "$(wc -l < "$LOG_FILE")" -gt 2000 ]; then
    tail -n 2000 "$LOG_FILE" > "${LOG_FILE}.tmp"
    mv "${LOG_FILE}.tmp" "$LOG_FILE"
    log "SYSTEM" "INFO " "日志裁剪完成，保留最新 2000 行"
fi

log "SYSTEM" "INFO " "===== 本轮养护会话结束 ====="
