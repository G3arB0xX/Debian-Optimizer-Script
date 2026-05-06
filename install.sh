#!/bin/bash
# =========================================================
# Debian Optimizer Script 一键安装自举脚本 (Bootstrap)
# =========================================================
set -euo pipefail

# 1. 权限拦截
if [[ $EUID -ne 0 ]]; then
    echo -e "\033[0;31m[错误] 权限不足，请使用 sudo 或 root 用户运行此脚本。\033[0m"
    exit 1
fi

# 2. 基础环境自举 (补齐 curl/git)
echo -e "\033[0;32m[1/4] 正在准备基础环境...\033[0m"
apt-get update -yq >/dev/null 2>&1
apt-get install -yq curl git >/dev/null 2>&1

# 3. 归属地探测 (智能路由选择)
echo -e "\033[0;32m[2/4] 正在检测网络环境...\033[0m"
IS_CN=false
# 使用多节点探测确保准确性
if [[ "$(curl -sL --connect-timeout 3 https://ipinfo.io/country 2>/dev/null)" == "CN" ]]; then
    IS_CN=true
elif curl -sL --connect-timeout 3 http://myip.ipip.net 2>/dev/null | grep -q "中国"; then
    IS_CN=true
fi

# 4. 仓库克隆与同步
INSTALL_DIR="/opt/debopti"
REPO_URL="https://github.com/G3arB0xX/Debian-Optimizer-Script.git"
CLONE_URL="$REPO_URL"

if [[ "$IS_CN" == "true" ]]; then
    echo -e "\033[1;33m[提示] 检测到中国大陆环境，已自动切换至 ghfast.top 加速镜像。\033[0m"
    CLONE_URL="https://ghfast.top/$REPO_URL"
fi

echo -e "\033[0;32m[3/4] 正在拉取项目资产至 $INSTALL_DIR ...\033[0m"
rm -rf "$INSTALL_DIR"
if ! git clone "$CLONE_URL" "$INSTALL_DIR"; then
    echo -e "\033[0;31m[错误] 仓库拉取失败，请检查网络连接。\033[0m"
    exit 1
fi

# 5. 移交控制权
echo -e "\033[0;32m[4/4] 正在启动主程序...\033[0m"
chmod +x "$INSTALL_DIR/deb_optimizer.sh"
cd "$INSTALL_DIR"

# 清理开发元数据，保持生产环境纯净
rm -rf .git .jj .github .gitignore VIBEINSTRCT.md

exec bash "$INSTALL_DIR/deb_optimizer.sh"
