#!/usr/bin/env bash
# ==============================================================================
# Debian Optimizer Script - GitHub Sanitized Push Tool (Advanced Edition)
# ==============================================================================
# 此脚本用于一键脱敏推送至 GitHub，移除 instructions/、tests/ 目录及相关指令文档。
# 整个重写过程在隔离的沙盒中进行，确保本地的 jj (jujutsu) 版本控制和工作区 100% 不受影响。
# ==============================================================================

set -euo pipefail

# 1. 基础配置
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
SANDBOX_DIR="${WORKSPACE_DIR}/.cache/push-temp"
SSH_CONTROL_PATH="${HOME}/.cache/debopti-ssh-control"
GITHUB_REMOTE="git@github.com:G3arB0xX/Debian-Optimizer-Script.git"
BRANCH_NAME="main"

# ================= GPG 重新签名配置 =================
# 如果您希望脱敏后的 Commit 在 GitHub 上依然带有 "Verified" 徽标，请将其设为 true。
# 注意：启用此项需要您的宿主机正在运行 gpg-agent 并且已缓存了 GPG 密钥的密码，
# 否则在重构 40+ 个 commit 时，系统会连续提示输入密码。
ENABLE_GPG_SIGNING=false
# ===================================================

FILTER_PATHS=(
    instructions
    tests
    vibeinstruction.md
    vibeinstruct.md
    VIBEINSTRCTION.md
    VIBEINSTRUCT.md
    VIBEINSTRCT.md
)

_ssh_agent_status() {
    # 返回值: 0=已有密钥, 1=agent 正常但无密钥, 2=无 agent 或 agent 不可用
    if [ -z "${SSH_AUTH_SOCK:-}" ]; then
        return 2
    fi
    if ssh-add -l >/dev/null 2>&1; then
        return 0
    fi
    local rc=$?
    if [ "${rc}" -eq 1 ]; then
        return 1
    fi
    return 2
}

_ensure_ssh_agent() {
    local status=0

    set +e
    _ssh_agent_status
    status=$?
    set -e

    if [ "${status}" -eq 0 ]; then
        return 0
    fi

    if [ "${status}" -eq 2 ]; then
        unset SSH_AUTH_SOCK SSH_AGENT_PID
        eval "$(ssh-agent -s)" >/dev/null
    fi
}

_resolve_github_identity() {
    local config="${HOME}/.ssh/config"
    local line host_pattern identity in_block=false

    if [ -f "${config}" ]; then
        while IFS= read -r line || [ -n "${line}" ]; do
            line="${line%%#*}"
            line="${line#"${line%%[![:space:]]*}"}"
            line="${line%"${line##*[![:space:]]}"}"
            [ -z "${line}" ] && continue

            if [[ "${line}" =~ ^[Hh]ost[[:space:]]+(.+)$ ]]; then
                host_pattern="${BASH_REMATCH[1]}"
                in_block=false
                if [[ " ${host_pattern} " == *" github.com "* ]] \
                    || [[ "${host_pattern}" == "github.com" ]]; then
                    in_block=true
                fi
                continue
            fi

            if [ "${in_block}" = true ] \
                && [[ "${line}" =~ ^[Ii]dentityFile[[:space:]]+(.+)$ ]]; then
                identity="${BASH_REMATCH[1]}"
                identity="${identity//\"/}"
                identity="${identity/#\~/${HOME}}"
                if [ -f "${identity}" ]; then
                    printf '%s' "${identity}"
                    return 0
                fi
            fi
        done < "${config}"
    fi

    for identity in \
        "${HOME}/.ssh/id_ed25519" \
        "${HOME}/.ssh/id_rsa" \
        "${HOME}/.ssh/id_ecdsa" \
        "${HOME}/.ssh/github_gbx_ed25519"; do
        if [ -f "${identity}" ]; then
            printf '%s' "${identity}"
            return 0
        fi
    done

    return 1
}

_identity_loaded() {
    local identity="$1"
    local fingerprint=""

    fingerprint="$(ssh-keygen -lf "${identity}" 2>/dev/null | awk '{print $2}')"
    [ -n "${fingerprint}" ] || return 1

    if ssh-add -l 2>/dev/null | grep -Fq "${fingerprint}"; then
        return 0
    fi
    return 1
}

_add_ssh_key_with_passphrase() {
    local identity="$1"
    local askpass_script=""

    askpass_script="$(mktemp "${TMPDIR:-/tmp}/debopti-ssh-askpass.XXXXXX")"
    cat > "${askpass_script}" << 'EOF'
#!/bin/sh
printf '%s\n' "${SSH_PASSPHRASE}"
EOF
    chmod 700 "${askpass_script}"
    # shellcheck disable=SC2064
    trap "rm -f '${askpass_script}'" RETURN

    DISPLAY="${DISPLAY:-:0}" SSH_ASKPASS="${askpass_script}" SSH_ASKPASS_REQUIRE=force \
        ssh-add "${identity}" </dev/null
}

_tty_usable() {
    [ -e /dev/tty ] 2>/dev/null || return 1
    ( exec 3</dev/tty ) 2>/dev/null || return 1
    exec 3<&-
    return 0
}

_load_ssh_key() {
    local identity="$1"
    local rc=0

    if [ -n "${SSH_PASSPHRASE:-}" ]; then
        echo "🔑 使用 SSH_PASSPHRASE 加载密钥（Cursor/AI 对话框模式）..."
        echo "   密钥文件: ${identity}"
        set +e
        _add_ssh_key_with_passphrase "${identity}"
        rc=$?
        set -e
        unset SSH_PASSPHRASE
        return "${rc}"
    fi

    if _tty_usable; then
        echo "🔑 请一次性输入 SSH 密钥密码（本次推送全程复用）..."
        echo "   密钥文件: ${identity}"
        unset SSH_ASKPASS SSH_ASKPASS_REQUIRE
        ssh-add "${identity}" </dev/tty
        return $?
    fi

    if [ "${PASSPHRASE_FROM_STDIN:-false}" = "true" ]; then
        echo "🔑 从标准输入读取 SSH 密钥密码..."
        echo "   密钥文件: ${identity}"
        if ! read -r SSH_PASSPHRASE; then
            echo "❌ 读取 SSH 密钥密码失败。"
            return 1
        fi
        export SSH_PASSPHRASE
        _load_ssh_key "${identity}"
        return $?
    fi

    echo "DEBOPTI_SSH_PASSPHRASE_REQUIRED"
    echo "❌ 当前环境无交互终端（如 Cursor Agent），无法直接提示输入密码。"
    echo ""
    echo "请在对话中提供 SSH 密钥密码后，由 Coder 执行："
    echo "  SSH_PASSPHRASE='<您的密码>' bash instructions/github-push.sh"
    echo ""
    echo "或在本地终端直接运行本脚本（将提示输入密码）。"
    echo "亦可通过管道：printf '%s\\n' '<密码>' | bash instructions/github-push.sh --passphrase-stdin"
    return 1
}

setup_ssh_once() {
    local identity=""

    mkdir -p "$(dirname "${SSH_CONTROL_PATH}")"
    _ensure_ssh_agent

    identity="$(_resolve_github_identity)" || {
        echo "❌ 未找到 GitHub SSH 私钥。"
        echo "   请在 ~/.ssh/config 中为 github.com 配置 IdentityFile，或将私钥放在 ~/.ssh/id_ed25519。"
        exit 1
    }

    set +e
    _identity_loaded "${identity}"
    local loaded=$?
    set -e

    if [ "${loaded}" -ne 0 ]; then
        if ! _load_ssh_key "${identity}"; then
            echo "❌ SSH 密钥加载失败，无法继续推送。"
            exit 1
        fi
    else
        echo "🔑 已检测到 GitHub SSH 密钥已加载，跳过重复输入。"
    fi

    export GIT_SSH_COMMAND="ssh -o ControlMaster=auto -o ControlPath=${SSH_CONTROL_PATH}-%r@%h-%p -o ControlPersist=600 -o IdentitiesOnly=yes -i ${identity}"
}

build_filter_index_cmd() {
    local cmd="git rm --cached -r --ignore-unmatch"
    local path
    for path in "${FILTER_PATHS[@]}"; do
        cmd+=" ${path}"
    done
    printf '%s' "${cmd}"
}

echo "========================================================"
echo "🛡️  开始执行 GitHub 历史脱敏安全推送流程..."
echo "========================================================"

# 解析命令行参数
FORCE_RUN=false
PASSPHRASE_FROM_STDIN=false
for arg in "$@"; do
    case "$arg" in
        -f|--force)
            FORCE_RUN=true
            ;;
        --passphrase-stdin)
            PASSPHRASE_FROM_STDIN=true
            ;;
    esac
done

setup_ssh_once

# 2. 检查是否有新提交需要推送 (智能跳过机制)
echo "🔍 正在检查本地与远程仓库的状态..."
if git fetch origin "${BRANCH_NAME}" --quiet 2>/dev/null; then
    LOCAL_HASH=$(git rev-parse "${BRANCH_NAME}")
    REMOTE_HASH=$(git rev-parse "origin/${BRANCH_NAME}" 2>/dev/null || echo "")

    if [ "${LOCAL_HASH}" = "${REMOTE_HASH}" ] && [ "${FORCE_RUN}" != "true" ]; then
        echo "✅ 本地分支与远程分支完全一致，没有检测到新提交，跳过推送。"
        echo "💡 提示：若远程仓库未脱敏且需要强制重写，请运行：$0 --force"
        echo "========================================================"
        exit 0
    fi
    if [ "${FORCE_RUN}" = "true" ]; then
        echo "💡 检测到 --force 参数，强制启动脱敏流程..."
    else
        echo "💡 检测到本地有未推送的更新，准备启动脱敏流程..."
    fi
else
    echo "⚠️  无法获取远程仓库状态（可能为首次推送或无网络），将直接启动脱敏重写..."
fi

# 3. 清理与异常捕获
cleanup() {
    if [ -d "${SANDBOX_DIR}" ]; then
        echo "🧹 安全清理沙盒临时目录..."
        rm -rf "${SANDBOX_DIR}"
    fi
    rm -f "${SSH_CONTROL_PATH}"-* 2>/dev/null || true
}
trap cleanup EXIT

# 4. 创建隔离沙盒克隆
echo "📦 正在创建本地 Git 阴影克隆 (隔离沙盒)..."
git clone --no-hardlinks "${WORKSPACE_DIR}" "${SANDBOX_DIR}"

# 5. 进入沙盒并执行脱敏重写
cd "${SANDBOX_DIR}"

# 强制检出需要推送的目标分支
echo "🎋 正在切换到 ${BRANCH_NAME} 分支..."
git checkout "${BRANCH_NAME}"

FILTER_INDEX_CMD="$(build_filter_index_cmd)"

echo "✂️ 正在重写 Git 提交历史，彻底移除敏感文件与文件夹..."
echo "   - 目标：instructions/ 文件夹"
echo "   - 目标：tests/ 文件夹"
echo "   - 目标：所有 vibeinstruction.md 变体"
echo "   - 状态：保留所有历史 Commit 结构，仅净化内容"

# 禁用 filter-branch 警告，保持输出纯净
export FILTER_BRANCH_SQUELCH_WARNING=1

if [ "${ENABLE_GPG_SIGNING}" = "true" ]; then
    echo "🔑 已启用 PGP 重新签名，正在重写并对 Commit 进行加密签名..."
    git filter-branch --force \
        --index-filter "${FILTER_INDEX_CMD}" \
        --commit-filter 'git commit-tree -S "$@"' \
        "${BRANCH_NAME}"
else
    git filter-branch --force \
        --index-filter "${FILTER_INDEX_CMD}" \
        "${BRANCH_NAME}"
fi

# 6. 添加 GitHub 远程源并推送
echo "🚀 正在向 GitHub 安全推送净化后的历史记录..."
git remote add github "${GITHUB_REMOTE}"

# 执行拉取以刷新本地对 github/main 的引用，避免 --force-with-lease 出现 stale info 报错
echo "📥 正在同步远程仓库状态..."
git fetch github "${BRANCH_NAME}" --quiet || true

# 使用安全强推，保护远程分支不受未知的覆盖风险
git push github "${BRANCH_NAME}" --force-with-lease

# 7. 清理现场
echo "🧹 推送成功！正在返回工作区..."
cd "${WORKSPACE_DIR}"

echo "========================================================"
echo "🎉 恭喜！脱敏推送成功完成！"
echo "ℹ️  GitHub 上的提交历史已完全净化，且本地 jj 版本控制完美保持原样。"
echo "========================================================"
