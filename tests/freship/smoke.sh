#!/bin/bash
# FreshIP 模块冒烟测试（不发起真实 Google 请求）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
LIB="${ROOT}/templates/apps/freship/freship_lib.sh"
FAIL=0

pass() { echo "  OK: $1"; }
fail() { echo "  FAIL: $1"; FAIL=1; }

echo "=== FreshIP smoke ==="

echo "[1] bash -n 语法检查"
for f in "${ROOT}"/templates/apps/freship/*.sh "${ROOT}/scripts/apps/freship.sh"; do
    if bash -n "$f"; then
        pass "$(basename "$f")"
    else
        fail "$(basename "$f")"
    fi
done

echo "[2] 探针判定逻辑"
# shellcheck source=/dev/null
source "$LIB"
REGION_CODE=JP
TARGET_CC=JP
PROBE_RC=0

evaluate_probe_score "US" "JP" "JP" || PROBE_RC=$?
[[ "$PROBE_SCORE" == "ok" && "$PROBE_RC" -eq 0 ]] && pass "YT 匹配视为 ok" || fail "YT 匹配视为 ok"

evaluate_probe_score "CN" "JP" "JP" || PROBE_RC=$?
[[ "$PROBE_SCORE" == "cn" ]] && pass "送中检测" || fail "送中检测"

TARGET_CC=US
REGION_CODE=US
evaluate_probe_score "JP" "JP" "JP" || PROBE_RC=$?
[[ "$PROBE_SCORE" == "drift" ]] && pass "漂移检测" || fail "漂移检测"

echo "[3] Persona 池（同 IP 限定 3 条 UA）"
TMP_UA=$(mktemp)
printf '%s\n' \
    'Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/1' \
    'Mozilla/5.0 (Linux; x86_64) Firefox/115' \
    'Mozilla/5.0 (Android 13; Mobile) Chrome/2' \
    'Mozilla/5.0 (Macintosh) Safari/3' \
    'Mozilla/5.0 (X11; Linux) Chrome/4' \
    > "$TMP_UA"
declare -A seen=()
for _ in {1..12}; do
    persona_from_ip "203.0.113.50" "$TMP_UA"
    seen["$PERSONA_UA"]=1
done
if [[ ${#seen[@]} -le 3 ]]; then
    pass "同 IP Persona 池 <= 3"
else
    fail "同 IP Persona 池 <= 3 (got ${#seen[@]})"
fi
rm -f "$TMP_UA"

echo "[4] 坐标抖动"
c1=$(get_random_coord "35.6812" 270)
c2=$(get_random_coord "35.6812" 270)
[[ "$c1" != "$c2" || "$c1" =~ ^35\. ]] && pass "get_random_coord 输出合法" || fail "get_random_coord"

echo "[5] parse_jump_gl"
[[ "$(parse_jump_gl 'https://www.google.co.jp/?gl=JP')" == "JP" ]] && pass "Jump gl 解析" || fail "Jump gl 解析"
[[ "$(parse_jump_gl 'https://www.google.cn/')" == "CN" ]] && pass "Jump 送中" || fail "Jump 送中"

echo "[6] maintain 每日跳过"
export INSTANCE_MODE=v4
sf=$(mktemp -d)
_freship_state_dir="$sf"
freship_state_file() { echo "${_freship_state_dir}/${INSTANCE_MODE}.state"; }
freship_write_state "RUN_MODE" "maintain"
freship_write_state "LAST_PROBE_UTC" "$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
if freship_should_skip_daily_maintain; then
    pass "maintain 同日跳过"
else
    fail "maintain 同日跳过"
fi
freship_write_state "LAST_PROBE_UTC" "2020-01-01T00:00:00Z"
if freship_should_skip_daily_maintain; then
    fail "maintain 跨日不跳过"
else
    pass "maintain 跨日不跳过"
fi
rm -rf "$sf"
unset _freship_state_dir

echo "[7] 日志行格式"
LOG_FILE=$(mktemp)
INSTANCE_MODE=v6 REGION_CODE=US
freship_log "RUNNER" "START" "启动养护任务 (活跃度: 37%)"
if grep -qE '^[0-9]{4}-[0-9]{2}-[0-9]{2} [0-9]{2}:[0-9]{2}:[0-9]{2} \[FreshIP\] 🚀 \| v6 \| US \| 启动养护任务' "$LOG_FILE"; then
    pass "file log format"
else
    fail "file log format"
fi
rm -f "$LOG_FILE"

if [[ "$FAIL" -eq 0 ]]; then
    echo "=== ALL PASSED ==="
    exit 0
fi
echo "=== FAILED ==="
exit 1
