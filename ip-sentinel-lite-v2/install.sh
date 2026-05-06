#!/bin/bash
# ==============================================================================
# IP-Sentinel 单机精简版 v2.0 - 安装脚本
# 新增：curl-impersonate 自动安装（TLS 指纹伪装）
# ==============================================================================

REPO_RAW="https://raw.githubusercontent.com/hotyue/IP-Sentinel/main"
INSTALL_DIR="/opt/ip_sentinel"
CONFIG_FILE="${INSTALL_DIR}/config.conf"

# curl-impersonate 预编译版本下载地址
# 项目地址：https://github.com/lwthiker/curl-impersonate
IMPERSONATE_VERSION="v0.6.1"
IMPERSONATE_BASE="https://github.com/lwthiker/curl-impersonate/releases/download/${IMPERSONATE_VERSION}"

echo "========================================================"
echo "      🛡️  IP-Sentinel 单机精简版 v2.0 - 安装向导"
echo "========================================================"

# ------------------------------------------------------------------------------
# 1. 依赖检查
# ------------------------------------------------------------------------------
echo -e "\n[1/7] 检查系统依赖..."
if [ -f /etc/debian_version ]; then
    apt-get update -y >/dev/null 2>&1
    apt-get install -y curl jq unzip file >/dev/null 2>&1
elif [ -f /etc/redhat-release ]; then
    yum install -y curl jq unzip file >/dev/null 2>&1
fi
echo "✅ 基础依赖完成"

# ------------------------------------------------------------------------------
# 2. curl-impersonate 安装
#
#    作用：让 curl 在建立 HTTPS 连接时，完整复制 Chrome 的 TLS 握手过程
#    （支持的加密套件列表、扩展顺序、椭圆曲线参数等），使得 JA3 指纹
#    与真实 Chrome 浏览器完全一致，通过 Cloudflare 等 Bot 检测。
#
#    安装到 /opt/ip_sentinel/bin/，不影响系统的 curl
# ------------------------------------------------------------------------------
echo -e "\n[2/7] 安装 curl-impersonate（TLS 指纹伪装引擎）..."

ARCH=$(uname -m)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
BIN_DIR="${INSTALL_DIR}/bin"
mkdir -p "$BIN_DIR"

# 检查是否已安装
if [ -x "${BIN_DIR}/curl_chrome124" ] || [ -x "${BIN_DIR}/curl_chrome116" ]; then
    echo "✅ curl-impersonate 已安装，跳过"
    IMPERSONATE_OK=true
else
    # 根据架构选择下载包
    case "$ARCH" in
        x86_64)  PKG_ARCH="x86_64" ;;
        aarch64) PKG_ARCH="aarch64" ;;
        armv7l)  PKG_ARCH="arm" ;;
        *)
            echo "⚠️ 架构 $ARCH 暂无预编译包，将跳过 TLS 伪装（使用系统 curl）"
            IMPERSONATE_OK=false
            PKG_ARCH=""
            ;;
    esac

    if [ -n "$PKG_ARCH" ]; then
        # 下载预编译的 Chrome 版本（仅 chrome 系，体积最小）
        DL_URL="${IMPERSONATE_BASE}/curl-impersonate-${IMPERSONATE_VERSION}.${OS}-${PKG_ARCH}.tar.gz"
        TMP_TAR="/tmp/curl_impersonate_$$.tar.gz"

        echo "  正在下载 curl-impersonate ${IMPERSONATE_VERSION} (${PKG_ARCH})..."
        if curl -sL --max-time 60 "$DL_URL" -o "$TMP_TAR" && [ -s "$TMP_TAR" ]; then
            tar -xzf "$TMP_TAR" -C "$BIN_DIR" --wildcards 'curl_chrome*' 2>/dev/null || \
            tar -xzf "$TMP_TAR" -C "$BIN_DIR" 2>/dev/null
            rm -f "$TMP_TAR"
            chmod +x "${BIN_DIR}/curl_chrome"* 2>/dev/null

            if [ -x "${BIN_DIR}/curl_chrome124" ] || [ -x "${BIN_DIR}/curl_chrome116" ]; then
                echo "✅ curl-impersonate 安装成功"
                IMPERSONATE_OK=true
            else
                echo "⚠️ 解压后未找到可执行文件，将使用系统 curl（TLS 指纹未伪装）"
                IMPERSONATE_OK=false
            fi
        else
            echo "⚠️ 下载失败（可能是网络问题），将使用系统 curl（TLS 指纹未伪装）"
            echo "   手动安装方法：https://github.com/lwthiker/curl-impersonate/releases"
            IMPERSONATE_OK=false
            rm -f "$TMP_TAR"
        fi
    fi
fi

# ------------------------------------------------------------------------------
# 3. 清理旧版
# ------------------------------------------------------------------------------
echo -e "\n[3/7] 清理旧版文件（保留日志和 bin 目录）..."
if [ -d "$INSTALL_DIR" ]; then
    rm -rf "${INSTALL_DIR}/core" \
           "${INSTALL_DIR}/data" \
           "${INSTALL_DIR}/config.conf" 2>/dev/null
fi
(crontab -l 2>/dev/null | grep -v "ip_sentinel") | crontab -
echo "✅ 清理完成"

# ------------------------------------------------------------------------------
# 4. 交互式城市选择
# ------------------------------------------------------------------------------
echo -e "\n[4/7] 拉取全球节点地图..."
curl -sL "${REPO_RAW}/data/map.json" -o "/tmp/ip_sentinel_map.json"
if [ ! -s "/tmp/ip_sentinel_map.json" ]; then
    echo "❌ 无法获取节点地图，请检查网络"
    exit 1
fi

echo -e "\n📍 请选择目标国家/地区："
mapfile -t COUNTRY_IDS   < <(jq -r '.continents[].countries[].id'           /tmp/ip_sentinel_map.json)
mapfile -t COUNTRY_NAMES < <(jq -r '.continents[].countries[].name'         /tmp/ip_sentinel_map.json)
mapfile -t COUNTRY_KW    < <(jq -r '.continents[].countries[].keyword_file' /tmp/ip_sentinel_map.json)

for i in "${!COUNTRY_IDS[@]}"; do
    printf "  %2d) %s\n" "$(( i+1 ))" "${COUNTRY_NAMES[$i]}"
done
read -rp "请输入序号（默认 1）: " C_SEL
C_SEL=$(( ${C_SEL:-1} - 1 ))
[ "$C_SEL" -lt 0 ] || [ "$C_SEL" -ge "${#COUNTRY_IDS[@]}" ] && C_SEL=0
COUNTRY_ID="${COUNTRY_IDS[$C_SEL]}"
KW_FILE="${COUNTRY_KW[$C_SEL]}"

echo -e "\n📍 请选择州/省："
mapfile -t STATE_IDS   < <(jq -r --arg c "$COUNTRY_ID" \
    '.continents[].countries[]|select(.id==$c)|.states[].id'   /tmp/ip_sentinel_map.json)
mapfile -t STATE_NAMES < <(jq -r --arg c "$COUNTRY_ID" \
    '.continents[].countries[]|select(.id==$c)|.states[].name' /tmp/ip_sentinel_map.json)

if [ "${#STATE_IDS[@]}" -eq 1 ]; then
    STATE_ID="${STATE_IDS[0]}"
    echo "  ↳ 自动选择唯一区域 [${STATE_NAMES[0]}]"
else
    for i in "${!STATE_IDS[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${STATE_NAMES[$i]}"; done
    read -rp "请输入序号（默认 1）: " S_SEL
    S_SEL=$(( ${S_SEL:-1} - 1 ))
    [ "$S_SEL" -lt 0 ] || [ "$S_SEL" -ge "${#STATE_IDS[@]}" ] && S_SEL=0
    STATE_ID="${STATE_IDS[$S_SEL]}"
fi

echo -e "\n📍 请选择城市："
mapfile -t CITY_IDS   < <(jq -r --arg c "$COUNTRY_ID" --arg s "$STATE_ID" \
    '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[].id' \
    /tmp/ip_sentinel_map.json)
mapfile -t CITY_NAMES < <(jq -r --arg c "$COUNTRY_ID" --arg s "$STATE_ID" \
    '.continents[].countries[]|select(.id==$c)|.states[]|select(.id==$s)|.cities[].name' \
    /tmp/ip_sentinel_map.json)

if [ "${#CITY_IDS[@]}" -eq 1 ]; then
    CITY_ID="${CITY_IDS[0]}"; CITY_NAME="${CITY_NAMES[0]}"
    echo "  ↳ 自动选择唯一城市 [${CITY_NAME}]"
else
    for i in "${!CITY_IDS[@]}"; do printf "  %2d) %s\n" "$(( i+1 ))" "${CITY_NAMES[$i]}"; done
    read -rp "请输入序号（默认 1）: " CI_SEL
    CI_SEL=$(( ${CI_SEL:-1} - 1 ))
    [ "$CI_SEL" -lt 0 ] || [ "$CI_SEL" -ge "${#CITY_IDS[@]}" ] && CI_SEL=0
    CITY_ID="${CITY_IDS[$CI_SEL]}"; CITY_NAME="${CITY_NAMES[$CI_SEL]}"
fi
rm -f /tmp/ip_sentinel_map.json
echo "✅ 已锁定节点：${CITY_NAME}"

# ------------------------------------------------------------------------------
# 5. 检测公网 IP
# ------------------------------------------------------------------------------
echo -e "\n[5/7] 检测本机公网 IP..."
DETECT_V4=$(curl -4 -s -m 5 api.ip.sb/ip 2>/dev/null | tr -d '[:space:]')
DETECT_V6=$(curl -6 -s -m 5 api.ip.sb/ip 2>/dev/null | tr -d '[:space:]')

if [ -n "$DETECT_V4" ] && [ -n "$DETECT_V6" ]; then
    echo "  1) IPv4: ${DETECT_V4}  ← 推荐"
    echo "  2) IPv6: ${DETECT_V6}"
    echo "  3) 手动输入"
    read -rp "请选择（默认 1）: " IP_CHOICE
    case "${IP_CHOICE:-1}" in
        2) BIND_IP="$DETECT_V6"; IP_PREF="6" ;;
        3) read -rp "请输入公网 IP: " BIND_IP
           [[ "$BIND_IP" == *":"* ]] && IP_PREF="6" || IP_PREF="4" ;;
        *) BIND_IP="$DETECT_V4"; IP_PREF="4" ;;
    esac
elif [ -n "$DETECT_V4" ]; then
    BIND_IP="$DETECT_V4"; IP_PREF="4"
    echo "  ↳ 检测到 IPv4: ${BIND_IP}"
elif [ -n "$DETECT_V6" ]; then
    BIND_IP="$DETECT_V6"; IP_PREF="6"
    echo "  ↳ 检测到 IPv6: ${BIND_IP}"
else
    read -rp "未能自动检测，请手动输入: " BIND_IP
    [[ "$BIND_IP" == *":"* ]] && IP_PREF="6" || IP_PREF="4"
fi
echo "✅ 锚点 IP: ${BIND_IP}"

# ------------------------------------------------------------------------------
# 6. Telegram（完全可选）
# ------------------------------------------------------------------------------
echo -e "\n[6/7] Telegram 推送（可选，直接回车跳过）"
read -rp "Bot Token（留空跳过）: " TG_TOKEN
TG_TOKEN="${TG_TOKEN:-}"
CHAT_ID=""
[ -n "$TG_TOKEN" ] && read -rp "Chat ID: " CHAT_ID

# ------------------------------------------------------------------------------
# 7. 部署文件
# ------------------------------------------------------------------------------
echo -e "\n[7/7] 部署文件与配置..."

mkdir -p "${INSTALL_DIR}/core"
mkdir -p "${INSTALL_DIR}/data/keywords"
mkdir -p "${INSTALL_DIR}/data/regions/${COUNTRY_ID}/${STATE_ID}"
mkdir -p "${INSTALL_DIR}/logs"

# 拉取区域 JSON、关键词库、UA 池
REGION_JSON="${INSTALL_DIR}/data/regions/${COUNTRY_ID}/${STATE_ID}/${CITY_ID}.json"
curl -sL "${REPO_RAW}/data/regions/${COUNTRY_ID}/${STATE_ID}/${CITY_ID}.json" -o "$REGION_JSON"
[ ! -s "$REGION_JSON" ] && echo "❌ 无法拉取城市规则文件" && exit 1
REGION_NAME=$(jq -r '.region_name' "$REGION_JSON")

curl -sL "${REPO_RAW}/data/keywords/${KW_FILE}" \
    -o "${INSTALL_DIR}/data/keywords/${KW_FILE}"
curl -sL "${REPO_RAW}/data/user_agents.txt" \
    -o "${INSTALL_DIR}/data/user_agents.txt"

# 复制脚本
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
for F in sentinel.sh updater.sh; do
    [ -f "${SCRIPT_DIR}/${F}" ] && cp "${SCRIPT_DIR}/${F}" "${INSTALL_DIR}/core/${F}"
done
chmod +x "${INSTALL_DIR}/core/"*.sh 2>/dev/null

# 写配置
cat > "$CONFIG_FILE" << EOF
# IP-Sentinel 单机精简版 v2.0 配置文件
# 生成：$(date '+%Y-%m-%d %H:%M:%S')

REGION_CODE="${COUNTRY_ID}"
REGION_NAME="${REGION_NAME}"
BIND_IP="${BIND_IP}"
IP_PREF="${IP_PREF}"
ENABLE_GOOGLE="true"
ENABLE_TRUST="true"
TG_TOKEN="${TG_TOKEN}"
CHAT_ID="${CHAT_ID}"
INSTALL_DIR="${INSTALL_DIR}"
LOG_FILE="${INSTALL_DIR}/logs/sentinel.log"
EOF
chmod 600 "$CONFIG_FILE"

# 注册 cron
(
    echo "*/20 * * * * bash ${INSTALL_DIR}/core/sentinel.sh >/dev/null 2>&1"
    echo "0 3 * * * bash ${INSTALL_DIR}/core/updater.sh >/dev/null 2>&1"
) | crontab -

echo "✅ 部署完成"
echo ""
echo "========================================================"
echo "🎉 IP-Sentinel v2.0 安装完成！"
echo ""
echo "  📍 养护区域：${REGION_NAME}"
echo "  🌐 绑定 IP ：${BIND_IP}"
echo "  🔒 TLS 伪装：$($IMPERSONATE_OK && echo '✅ curl-impersonate 已启用' || echo '⚠️ 未启用（见上方说明）')"
echo "  ⏰ 运行频率：每20分钟（深夜0~6点静默）"
echo ""
echo "  📋 查看日志：tail -f ${INSTALL_DIR}/logs/sentinel.log"
echo "  🔧 修改配置：nano ${CONFIG_FILE}"
echo ""
echo "  卸载："
echo "  (crontab -l | grep -v ip_sentinel) | crontab -"
echo "  rm -rf ${INSTALL_DIR}"
echo "========================================================"
