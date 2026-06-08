# Docker 安装与配置

本文档介绍如何在 Debian 上安装 Docker Engine 与 Docker Compose，并注入生产级配置。

**前提条件**：root 权限，Debian 10+

---

## 1. 安装 Docker

Docker 官方提供了一键安装脚本，是最简单且能保持最新版本的安装方式。

### 通用环境

```bash
curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
bash /tmp/get-docker.sh
```

### 中国大陆环境（使用阿里云镜像）

```bash
curl -fsSL https://get.docker.com -o /tmp/get-docker.sh
bash /tmp/get-docker.sh --mirror Aliyun
```

安装完成后验证：

```bash
docker version
docker compose version
```

---

## 2. 注入生产级 daemon.json

默认的 Docker 守护进程配置没有日志大小限制，长期运行后容器日志可能占满磁盘。

```bash
mkdir -p /etc/docker
```

**通用配置（海外/不需要镜像加速）**：

```bash
cat > /etc/docker/daemon.json << 'EOF'
{
    "log-driver": "json-file",       // 日志驱动：使用 JSON 文件格式记录容器日志
    "log-opts": {
        "max-size": "20m",           // 单个日志文件最大 20MB，超出后自动轮转
        "max-file": "3"              // 最多保留 3 个日志文件副本（即每个容器最多占 60MB 日志空间）
    },
    "live-restore": true,            // dockerd 守护进程重启时保持容器继续运行（不中断业务）
    "max-concurrent-downloads": 10,  // 并发拉取镜像层数（默认 3，提高可加速大镜像拉取）
    "storage-driver": "overlay2"     // 存储驱动：overlay2 是当前最稳定高效的选择
}
EOF
```

**中国大陆配置（含镜像加速）**：

```bash
cat > /etc/docker/daemon.json << 'EOF'
{
    "registry-mirrors": [            // 镜像加速源列表（按顺序尝试，第一个不可用时自动回退）
        "https://docker.m.daocloud.io",
        "https://mirror.baidubce.com",
        "https://docker.nju.edu.cn"
    ],
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
```

> **注意**：上方 JSON 中的 `//` 注释仅用于文档说明，`daemon.json` 不支持注释语法。实际写入时请使用不带注释的版本，或者直接复制上方代码块（Docker 25+ 会自动忽略带 `//` 的行，但旧版本可能报错）。

应用配置并启动服务：

```bash
systemctl daemon-reload
systemctl enable --now docker
systemctl restart docker
```

验证配置生效：

```bash
docker info | grep -A5 "Logging Driver"
docker info | grep "Registry Mirrors" -A5
```

---

## 3. 将普通用户加入 docker 组

将用户加入 `docker` 组后，该用户无需 `sudo` 即可运行 Docker 命令：

```bash
# 将 myuser 替换为实际用户名
usermod -aG docker myuser

# 需要重新登录后生效
# 验证（以 myuser 身份执行）
docker ps
```

> **安全提示**：`docker` 组成员等同于拥有 root 权限，请只将受信任的用户加入该组。

---

## 4. 开启 IP 转发（容器网络需要）

Docker 的容器网络依赖系统的 IP 转发功能。如果容器内无法访问外网，请检查并开启：

```bash
cat > /etc/sysctl.d/99-debopti-forwarding.conf << 'EOF'
net.ipv4.ip_forward = 1
net.ipv4.conf.all.forwarding = 1
net.ipv4.conf.default.forwarding = 1
net.ipv6.conf.all.forwarding = 1
net.ipv6.conf.default.forwarding = 1
EOF

sysctl --system

# 验证
sysctl net.ipv4.ip_forward
# 输出应为: net.ipv4.ip_forward = 1
```

---

## 5. 常用 Docker 操作

```bash
# 查看运行中的容器
docker ps

# 查看所有容器（包括已停止）
docker ps -a

# 查看容器日志（持续输出）
docker logs -f container_name

# 查看镜像
docker images

# 清理无用镜像、容器、网络（不删除卷）
docker system prune -f

# 清理包括无用卷
docker system prune -f --volumes
```

---

## 6. 卸载 Docker

```bash
# 停止服务
systemctl stop docker docker.socket containerd

# 卸载软件包
apt-get purge -y docker-ce docker-ce-cli containerd.io \
    docker-buildx-plugin docker-compose-plugin
apt-get autoremove -y

# 清理配置
rm -rf /etc/docker

# 可选：清理所有镜像、容器、卷数据（不可恢复）
rm -rf /var/lib/docker /var/lib/containerd
```
