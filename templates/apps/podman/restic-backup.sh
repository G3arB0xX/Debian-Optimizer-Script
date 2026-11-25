#!/usr/bin/env bash
# Restic 弹性备份包装脚本 — apps 用户 rootless 容器环境
# 路径: /home/apps/.local/bin/restic-backup.sh
# 环境变量由 systemd EnvironmentFile / LoadCredential 或 sudo -u apps env 注入，禁止硬编码凭据。

set -Eeuo pipefail

on_err() {
    local line="$1"
    local rc="$2"
    printf '[ERROR] restic-backup.sh aborted at line %s (exit code %s)\n' "$line" "$rc" >&2
}

trap 'on_err "$LINENO" "$?"' ERR

RESTIC_BIN="${RESTIC_BIN:-restic}"
RESTIC_BACKUP_TARGET="${RESTIC_BACKUP_TARGET:-/home/apps}"
RESTIC_EXCLUDE_FILE="${RESTIC_EXCLUDE_FILE:-/home/apps/.config/restic-excludes}"
RESTIC_KEEP_DAILY="${RESTIC_KEEP_DAILY:-7}"
RESTIC_KEEP_WEEKLY="${RESTIC_KEEP_WEEKLY:-4}"
RESTIC_KEEP_MONTHLY="${RESTIC_KEEP_MONTHLY:-6}"
RESTIC_SKIP_PRUNE="${RESTIC_SKIP_PRUNE:-0}"

backup_rc=0
"$RESTIC_BIN" backup "$RESTIC_BACKUP_TARGET" --exclude-file="$RESTIC_EXCLUDE_FILE" "$@" || backup_rc=$?

if [[ "$backup_rc" -eq 0 ]]; then
    printf '[INFO] restic backup completed cleanly for %s\n' "$RESTIC_BACKUP_TARGET" >&2
elif [[ "$backup_rc" -eq 3 ]]; then
    printf '[WARN] restic backup exited with code 3 (partial read error, e.g. dynamic 0700 dirs); snapshot created, continuing to forget --prune\n' >&2
else
    printf '[ERROR] restic backup failed fatally with exit code %s; aborting before forget --prune\n' "$backup_rc" >&2
    exit "$backup_rc"
fi

if [[ "$RESTIC_SKIP_PRUNE" != "1" ]]; then
    "$RESTIC_BIN" forget --prune \
        --keep-daily "$RESTIC_KEEP_DAILY" \
        --keep-weekly "$RESTIC_KEEP_WEEKLY" \
        --keep-monthly "$RESTIC_KEEP_MONTHLY"
fi
