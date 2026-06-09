# debopti Podman 模块测试基础镜像
# 预装通用依赖，跳过每次测试重复 apt 安装 systemd/sudo 等
ARG DEBIAN_TAG=bookworm
FROM docker.io/library/debian:${DEBIAN_TAG}

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        systemd systemd-sysv dbus sudo acl curl ca-certificates util-linux \
    && rm -rf /var/lib/apt/lists/*

CMD ["sleep", "infinity"]
