#!/bin/bash
# =========================================================
# Debian Optimizer Script 一键安装自举脚本 (Bootstrap)
# =========================================================
set -euo pipefail

# 1. 权限拦截与自动提权
if [[ $EUID -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1; then
        echo -e "\033[1;33m⚠️  当前非 root 权限，正在尝试通过 sudo 自动提权...\033[0m"
        if [[ -f "$0" ]]; then
            exec sudo bash "$0" "$@"
        else
            # 管道运行模式，智能识别 curl 与 wget 进行提权拉取
            if command -v curl >/dev/null 2>&1; then
                exec sudo bash -c "$(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
            elif command -v wget >/dev/null 2>&1; then
                exec sudo bash -c "$(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)"
            else
                echo -e "\033[0;31m[错误] 权限不足且缺失 curl 和 wget，无法自动提权。请使用 root 用户手动执行。\033[0m"
                exit 1
            fi
        fi
    else
        echo -e "\033[0;31m[错误] 权限不足，且系统未安装 sudo。\033[0m"
        echo -e "\033[1;32m请使用以下方法之一运行：\033[0m"
        echo -e "  1. 切换到 root 用户运行（推荐）：\033[1;36msu - -c \"bash <(curl -fsSL https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)\"\033[0m"
        echo -e "     或（若无 curl）：\033[1;36msu - -c \"bash <(wget -qO- https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/install.sh)\"\033[0m"
        echo -e "  2. 安装 sudo 并将当前用户加入 sudoers 组后再试。"
        exit 1
    fi
fi

# 2. 基础环境自举 (补齐 curl/git/wget)
echo -e "\033[0;32m[1/4] 正在检查基础环境...\033[0m"

# 临时 DNS 修复逻辑：若当前 DNS 无法解析 github 域名，写入临时 DNS (1.1.1.1/8.8.8.8) 恢复连接
if ! getent hosts raw.githubusercontent.com >/dev/null 2>&1 && ! getent hosts github.com >/dev/null 2>&1; then
    echo -e "\033[1;33m⚠️  检测到 DNS 解析故障，正在写入临时公共 DNS (1.1.1.1/8.8.8.8) 恢复连接...\033[0m"
    if [[ -f /etc/resolv.conf && ! -f /etc/resolv.conf.debopti.bak ]]; then
        cp /etc/resolv.conf /etc/resolv.conf.debopti.bak
    fi
    cat > /etc/resolv.conf << 'EOF'
nameserver 1.1.1.1
nameserver 8.8.8.8
EOF
fi

# 智能补齐基础工具
if ! command -v curl >/dev/null 2>&1 || ! command -v git >/dev/null 2>&1 || ! command -v wget >/dev/null 2>&1; then
    echo -e "\033[0;32m检测到系统缺少基础工具 (curl/git/wget)，正在安装...\033[0m"
    apt-get update -yq >/dev/null 2>&1 || true
    if ! apt-get install -yq curl git wget >/dev/null 2>&1; then
        echo -e "\033[1;33m⚠️  标准安装失败，尝试通过 --fix-missing 修复安装...\033[0m"
        apt-get install -yq --fix-missing curl git wget >/dev/null 2>&1 || {
            echo -e "\033[0;31m❌ 基础工具 (curl/git/wget) 安装失败，请检查您的网络连接与软件源配置。\033[0m"
            exit 1
        }
    fi
else
    echo -e "\033[0;32m基础工具 (curl/git/wget) 已就绪，跳过安装阶段。\033[0m"
fi

# 3. 归属地探测 (智能路由选择)
echo -e "\033[0;32m[2/4] 正在检测网络环境...\033[0m"
IS_CN=false

fetch_url() {
    local url=$1
    if command -v curl >/dev/null 2>&1; then
        curl -sL --connect-timeout 3 "$url" 2>/dev/null || true
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- --timeout=3 "$url" 2>/dev/null || true
    fi
}

if [[ "$(fetch_url https://ipinfo.io/country)" == "CN" ]]; then
    IS_CN=true
elif fetch_url http://myip.ipip.net | grep -q "中国"; then
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
