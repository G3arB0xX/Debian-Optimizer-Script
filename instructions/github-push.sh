#!/usr/bin/env bash
# ==============================================================================
# Debian Optimizer Script - GitHub Sanitized Push Tool (Advanced Edition)
# ==============================================================================
# 此脚本用于一键脱敏推送至 GitHub，移除所有的 instructions 和 scratch 目录及相关指令文档。
# 整个重写过程在隔离的沙盒中进行，确保本地的 jj (jujutsu) 版本控制和工作区 100% 不受影响。
# ==============================================================================

set -euo pipefail

# 1. 基础配置
WORKSPACE_DIR="/home/user/文档/CodeDev/deboptiscript"
SANDBOX_DIR="${WORKSPACE_DIR}/scratch/.push-temp"
GITHUB_REMOTE="git@github.com:G3arB0xX/Debian-Optimizer-Script.git"
BRANCH_NAME="main"

# ================= GPG 重新签名配置 =================
# 如果您希望脱敏后的 Commit 在 GitHub 上依然带有 "Verified" 徽标，请将其设为 true。
# 注意：启用此项需要您的宿主机正在运行 gpg-agent 并且已缓存了 GPG 密钥的密码，
# 否则在重构 40+ 个 commit 时，系统会连续提示输入密码。
ENABLE_GPG_SIGNING=false
# ===================================================

echo "========================================================"
echo "🛡️  开始执行 GitHub 历史脱敏安全推送流程..."
echo "========================================================"

# 解析命令行参数
FORCE_RUN=false
for arg in "$@"; do
    case "$arg" in
        -f|--force)
            FORCE_RUN=true
            ;;
    esac
done

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

echo "✂️ 正在重写 Git 提交历史，彻底移除敏感文件与文件夹..."
echo "   - 目标：instructions/ 文件夹"
echo "   - 目标：scratch/ 文件夹"
echo "   - 目标：所有 vibeinstruction.md 变体"
echo "   - 状态：保留所有历史 Commit 结构，仅净化内容"

# 禁用 filter-branch 警告，保持输出纯净
export FILTER_BRANCH_SQUELCH_WARNING=1

if [ "${ENABLE_GPG_SIGNING}" = "true" ]; then
    echo "🔑 已启用 PGP 重新签名，正在重写并对 Commit 进行加密签名..."
    git filter-branch --force \
        --index-filter 'git rm --cached -r --ignore-unmatch instructions scratch vibeinstruction.md vibeinstruct.md VIBEINSTRCTION.md VIBEINSTRUCT.md VIBEINSTRCT.md' \
        --commit-filter 'git commit-tree -S "$@"' \
        "${BRANCH_NAME}"
else
    git filter-branch --force \
        --index-filter 'git rm --cached -r --ignore-unmatch instructions scratch vibeinstruction.md vibeinstruct.md VIBEINSTRCTION.md VIBEINSTRUCT.md VIBEINSTRCT.md' \
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
