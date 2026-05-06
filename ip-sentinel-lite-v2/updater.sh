#!/bin/bash
# ==============================================================================
# IP-Sentinel 单机精简版 v2.0 - 数据更新器 updater.sh
# 功能：
#   1. 每日同步最新热搜关键词（滑动窗口，保留最新200条）
#   2. 每日同步区域 JSON（white_urls 每天由 GitHub Actions 刷入当日新闻链接）
#   3. 每30天更新一次 UA 指纹池
#   4. 日志清理（保留最新2000行）
# 调用方：cron，每周日凌晨3点（也可每天运行）
# ==============================================================================

INSTALL_DIR="/opt/ip_sentinel"
CONFIG_FILE="${INSTALL_DIR}/config.conf"
REPO_RAW="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
UA_TIMESTAMP="${INSTALL_DIR}/core/.ua_last_update"

if [ ! -f "$CONFIG_FILE" ]; then exit 1; fi
source "$CONFIG_FILE"

LOG_FILE="${INSTALL_DIR}/logs/sentinel.log"

log() {
    mkdir -p "${INSTALL_DIR}/logs"
    printf "[%s UTC] [Updater] [%-5s] [%s] %s\n" \
        "$(date -u '+%Y-%m-%d %H:%M:%S')" "$1" "${REGION_CODE:-??}" "$2" \
        >> "$LOG_FILE"
}

log "INFO " "===== OTA 数据更新开始 ====="

# curl 基础参数（绑定出口 IP，与 sentinel.sh 保持一致）
CURL_BASE="curl -${IP_PREF:-4} -sL --max-time 20"
if [ -n "$BIND_IP" ]; then
    RAW_IP=$(echo "$BIND_IP" | tr -d '[]')
    if ip addr show 2>/dev/null | grep -qw "$RAW_IP"; then
        CURL_BASE="$CURL_BASE --interface $RAW_IP"
    fi
fi

# ------------------------------------------------------------------------------
# 1. 每日同步热搜关键词
#    原项目用 GitHub Actions + fetch_trends.py 每天更新词库并推送到仓库
#    这里直接拉取仓库里已经更新好的文件即可
# ------------------------------------------------------------------------------
TMP_KW="/tmp/ip_sentinel_kw_$$.txt"
$CURL_BASE "${REPO_RAW}/data/keywords/kw_${REGION_CODE}.txt" -o "$TMP_KW"

if [ -s "$TMP_KW" ]; then
    mv "$TMP_KW" "${INSTALL_DIR}/data/keywords/kw_${REGION_CODE}.txt"
    KW_COUNT=$(wc -l < "${INSTALL_DIR}/data/keywords/kw_${REGION_CODE}.txt")
    log "INFO " "✅ 关键词库已同步（kw_${REGION_CODE}.txt，共 ${KW_COUNT} 条）"
else
    log "WARN " "⚠️ 关键词库拉取失败，保留本地旧数据"
    rm -f "$TMP_KW"
fi

# ------------------------------------------------------------------------------
# 2. 每日同步区域 JSON
#    原项目用 GitHub Actions + fetch_trust_urls.py 每天把当日新闻 URL 注入
#    white_urls，所以这里需要每天拉取以保证 white_urls 内容是"今天的新闻"
# ------------------------------------------------------------------------------
REGION_JSON=$(find "${INSTALL_DIR}/data/regions" -name "*.json" 2>/dev/null | head -n 1)

if [ -n "$REGION_JSON" ] && [ -f "$REGION_JSON" ]; then
    # 提取该文件相对于 INSTALL_DIR 的路径（用于拼接远端 URL）
    REL_PATH="${REGION_JSON#${INSTALL_DIR}/}"
    TMP_JSON="/tmp/ip_sentinel_region_$$.json"

    $CURL_BASE "${REPO_RAW}/${REL_PATH}" -o "$TMP_JSON"

    if [ -s "$TMP_JSON" ] && jq . "$TMP_JSON" >/dev/null 2>&1; then
        mv "$TMP_JSON" "$REGION_JSON"
        WHITE_COUNT=$(jq '.trust_module.white_urls | length' "$REGION_JSON")
        log "INFO " "✅ 区域规则已同步（${REL_PATH}，white_urls 共 ${WHITE_COUNT} 条）"
    else
        log "WARN " "⚠️ 区域 JSON 拉取失败或格式损坏，保留本地旧数据"
        rm -f "$TMP_JSON"
    fi
else
    log "WARN " "⚠️ 未找到本地区域 JSON 文件，跳过同步"
fi

# ------------------------------------------------------------------------------
# 3. UA 指纹池：每30天更新一次（原项目逻辑）
#    UA 池约4000条，更新太频繁没有意义，30天换一批足够
# ------------------------------------------------------------------------------
NOW=$(date +%s)
LAST_UA=0

if [ -f "$UA_TIMESTAMP" ]; then
    LAST_UA=$(cat "$UA_TIMESTAMP" | tr -d '[:space:]')
    [[ ! "$LAST_UA" =~ ^[0-9]+$ ]] && LAST_UA=0
fi

DIFF=$(( NOW - LAST_UA ))

if [ "$DIFF" -ge 2592000 ] || [ "$LAST_UA" -eq 0 ]; then
    TMP_UA="/tmp/ip_sentinel_ua_$$.txt"
    $CURL_BASE "${REPO_RAW}/data/user_agents.txt" -o "$TMP_UA"

    if [ -s "$TMP_UA" ]; then
        mv "$TMP_UA" "${INSTALL_DIR}/data/user_agents.txt"
        echo "$NOW" > "$UA_TIMESTAMP"
        UA_COUNT=$(wc -l < "${INSTALL_DIR}/data/user_agents.txt")
        log "INFO " "✅ UA 指纹池已更新（共 ${UA_COUNT} 条）"
    else
        log "WARN " "⚠️ UA 池拉取失败，保留本地旧数据"
        rm -f "$TMP_UA"
    fi
else
    DAYS_LEFT=$(( (2592000 - DIFF) / 86400 ))
    log "INFO " "⏳ UA 指纹池处于30天静默期（距下次更新约 ${DAYS_LEFT} 天）"
fi

# ------------------------------------------------------------------------------
# 4. 日志裁剪（保留最新2000行）
# ------------------------------------------------------------------------------
if [ -f "$LOG_FILE" ] && [ "$(wc -l < "$LOG_FILE")" -gt 2000 ]; then
    tail -n 2000 "$LOG_FILE" > "${LOG_FILE}.tmp"
    mv "${LOG_FILE}.tmp" "$LOG_FILE"
    log "INFO " "🧹 日志已裁剪，保留最新2000行"
fi

log "INFO " "===== OTA 数据更新结束 ====="
