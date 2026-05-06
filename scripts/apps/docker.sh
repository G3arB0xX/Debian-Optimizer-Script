#!/bin/bash
# =========================================================
# Docker Engine 与 Compose 生产环境部署模块
# =========================================================

install_docker() {
    info "正在部署 Docker 生产级环境 (Engine + Compose)..."
    
    # 获取官方自动化脚本
    download_with_fallback "/tmp/get-docker.sh" "https://raw.githubusercontent.com/docker/docker-install/master/install.sh" || return 1
    
    # 针对国内环境切换阿里云镜像加速通道
    if [[ "$IS_CN_REGION" == "true" ]]; then
        info "国内环境检测：切换至 Aliyun Docker 镜像源..."
        bash /tmp/get-docker.sh --mirror Aliyun || return 1
    else
        bash /tmp/get-docker.sh || return 1
    fi
    
    # --- 生产级 daemon.json 优化 ---
    info "正在注入 Docker 守护进程优化配置 (日志轮转/存储引擎/并发限制)..."
    mkdir -p /etc/docker
    
    local registry_json=""
    # 国内环境注入常用的 Hub 镜像加速地址，防范 Docker Hub 封锁
    if [[ "$IS_CN_REGION" == "true" ]]; then
        registry_json='"registry-mirrors": ["https://docker.m.daocloud.io", "https://mirror.baidubce.com", "https://docker.nju.edu.cn"],'
    fi
    
    cat > /etc/docker/daemon.json << EOF
{
    ${registry_json}
    "log-driver": "json-file",
    "log-opts": {
        "max-size": "20m",
        "max-file": "3"
    },
    "live-restore": true,
    "max-concurrent-downloads": 10,
    "storage-driver": "overlay2"
}
EOF
    # 配置解读：
    # 1. log-opts: 限制单个容器日志上限 20MB，保留 3 个滚动副本，防止日志无限膨胀导致磁盘爆满。
    # 2. live-restore: 当 dockerd 升级或崩溃时，保持容器进程继续运行。
    # 3. max-concurrent-downloads: 提升镜像拉取并发度，极大提升复杂项目的构建速度。

    systemctl daemon-reload
    systemctl enable --now docker
    
    if command -v docker >/dev/null 2>&1; then
        success "Docker 环境部署成功！"
        docker compose version
        warn "提醒：部分容器网络可能需要开启系统的 IP 转发 (TUI 选项 2) 才能正常通信。"
    else
        err "Docker 安装失败，请检查网络或系统资源。"
        return 1
    fi
}

uninstall_docker() {
    info "准备卸载 Docker 环境..."
    
    echo -e "${RED}警告：即将删除 Docker 核心程序及其所有配置文件！${NC}"
    read -p "是否同步清除所有的业务数据 (镜像、容器、卷)？ [y/N]: " delete_data
    
    # 优雅停止服务
    systemctl stop docker docker.socket containerd >/dev/null 2>&1
    
    # 彻底卸载程序包
    apt-get purge -yq docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin >/dev/null 2>&1
    apt-get autoremove -yq >/dev/null 2>&1
    
    # 清理残留目录
    rm -rf /etc/docker /usr/libexec/docker /var/run/docker.sock /usr/local/bin/docker-compose
    
    # 根据用户确认清理数据目录
    if [[ "$delete_data" =~ ^[Yy]$ ]]; then
        info "正在抹除持久化数据目录 (/var/lib/docker)..."
        rm -rf /var/lib/docker /var/lib/containerd
    else
        info "数据卷已保留在 /var/lib/docker，下次重装可自动恢复。"
    fi
    
    success "Docker 已卸载。"
}
