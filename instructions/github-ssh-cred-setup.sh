#!/usr/bin/env bash
# 将 GitHub SSH 密钥密码写入 systemd 用户凭证（加密存储于 ~/.config/credstore/）。
# 供 instructions/github-push.sh 自动读取，避免每次在对话或终端输入。
#
# 用法:
#   bash instructions/github-ssh-cred-setup.sh              # 交互输入（不回显）
#   bash instructions/github-ssh-cred-setup.sh --passphrase-stdin  # 从 stdin 读一行
#   printf '%s\n' '密码' | bash instructions/github-ssh-cred-setup.sh --passphrase-stdin

set -euo pipefail

CRED_NAME="debopti-github-ssh"
CRED_DIR="${XDG_CONFIG_HOME:-${HOME}/.config}/credstore"
CRED_FILE="${CRED_DIR}/${CRED_NAME}.cred"

if ! command -v systemd-creds >/dev/null 2>&1; then
    echo "❌ 未找到 systemd-creds（需要 systemd ≥ 256）。"
    exit 1
fi

PASSPHRASE_FROM_STDIN=false
for arg in "$@"; do
    case "$arg" in
        --passphrase-stdin) PASSPHRASE_FROM_STDIN=true ;;
        -h|--help)
            sed -n '1,12p' "$0"
            exit 0
            ;;
    esac
done

if [ "${PASSPHRASE_FROM_STDIN}" = "true" ]; then
    if ! IFS= read -r pass; then
        echo "❌ 读取密码失败。"
        exit 1
    fi
else
    if [ -t 0 ] && [ -t 1 ]; then
        read -r -s -p "GitHub SSH 密钥密码: " pass
        echo ""
        read -r -s -p "再次确认: " pass2
        echo ""
        if [ "${pass}" != "${pass2}" ]; then
            echo "❌ 两次输入不一致。"
            exit 1
        fi
    else
        echo "❌ 非交互环境请使用: printf '%s\\n' '<密码>' | bash $0 --passphrase-stdin"
        exit 1
    fi
fi

if [ -z "${pass}" ]; then
    echo "❌ 密码不能为空。"
    exit 1
fi

mkdir -p "${CRED_DIR}"
chmod 700 "${CRED_DIR}"

tmp_plain="$(mktemp "${TMPDIR:-/tmp}/debopti-ssh-plain.XXXXXX")"
chmod 600 "${tmp_plain}"
printf '%s' "${pass}" > "${tmp_plain}"
unset pass pass2

if [ -f "${CRED_FILE}" ]; then
    backup="${CRED_FILE}.bak.$(date +%Y%m%d%H%M%S)"
    cp -a "${CRED_FILE}" "${backup}"
    echo "ℹ️  已备份旧凭证: ${backup}"
fi

systemd-creds --user encrypt "${tmp_plain}" "${CRED_FILE}"
rm -f "${tmp_plain}"
chmod 600 "${CRED_FILE}"

if ! systemd-creds --user decrypt "${CRED_FILE}" - >/dev/null 2>&1; then
    echo "❌ 凭证写入后自检解密失败，请检查 systemd-creds --user 是否可用。"
    exit 1
fi

echo "✅ GitHub SSH 密码已加密保存: ${CRED_FILE}"
echo "   github-push.sh 将自动从此凭证加载（无需 SSH_PASSPHRASE 环境变量）。"
