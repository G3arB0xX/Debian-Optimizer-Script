#!/bin/bash
# FreshIP 主调度：探针优先 + maintain/simulate 状态机

export PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin

CONFIG_FILE="/etc/freship/freship.conf"
[[ -f "$CONFIG_FILE" ]] || exit 1
# shellcheck source=/dev/null
source "$CONFIG_FILE"

INSTANCE_MODE=${1:-v4}
CORE_DIR="${INSTALL_DIR}/core"
# shellcheck source=/dev/null
source "${CORE_DIR}/freship_lib.sh"

BIND_IP=""
[[ "$INSTANCE_MODE" == "v4" ]] && BIND_IP="${BIND_IPV4:-}"
[[ "$INSTANCE_MODE" == "v6" ]] && BIND_IP="${BIND_IPV6:-}"
[[ -z "$BIND_IP" ]] && exit 1

RUNNER_LOCK="/tmp/freship_${INSTANCE_MODE}.runner.lock"
exec 300>"$RUNNER_LOCK"
if ! flock -n 300; then
    freship_log "RUNNER" "WARN" "上一轮任务尚未结束，跳过"
    exit 0
fi

if freship_should_skip_quiet_hours; then
    local_hour=$(date -u -d "${UTC_OFFSET:-+0} hours" +%H 2>/dev/null || date +%H)
    freship_log "RUNNER" "SLEEP" "目标地区深夜 (${local_hour}:00)，跳过"
    exit 0
fi

if [[ "${CI:-}" != "true" ]] && freship_should_skip_low_activity; then
    daily_seed=$(echo "$(date +%Y%m%d)" | cksum | awk '{print $1}')
    activity=$(( daily_seed % 100 ))
    freship_log "RUNNER" "INFO" "今日活跃度低 (${activity}%)，跳过"
    exit 0
fi

export INSTANCE_MODE BIND_IP

# 防并发抖动（非交互终端；CI/测试跳过）
if [[ ! -t 1 && "${CI:-}" != "true" ]]; then
    jitter=$(( RANDOM % 180 ))
    freship_log "RUNNER" "INFO" "随机抖动 ${jitter}s"
    sleep "$jitter"
fi

freship_log "RUNNER" "START" "实例 ${INSTANCE_MODE} 启动 | 出口 ${BIND_IP}"

PROBE_RC=0
bash "${CORE_DIR}/freship_probe.sh" || PROBE_RC=$?

if [[ "$PROBE_RC" -eq 0 ]]; then
    freship_log "RUNNER" "INFO" "区域达标，进入 maintain（仅自检），跳过模拟流量"
    exit 0
fi

freship_log "RUNNER" "INFO" "区域未达标 (rc=${PROBE_RC})，进入 simulate 模拟养护"

module_roll=$(( RANDOM % 100 + 1 ))
if [[ "$module_roll" -le 70 ]]; then
    nice -n 19 bash "${CORE_DIR}/freship_mod_google.sh" 300>&- || true
else
    nice -n 19 bash "${CORE_DIR}/freship_mod_trust.sh" 300>&- || true
fi

bash "${CORE_DIR}/freship_probe.sh" || true
freship_log "RUNNER" "END" "本轮调度结束"
exit 0
