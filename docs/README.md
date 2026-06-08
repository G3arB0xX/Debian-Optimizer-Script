# Debian Optimizer Script — 手动操作文档

本目录包含项目各功能模块的手动操作教程。

每篇文档对应一个功能模块，记录了脚本自动执行的完整步骤，让你可以按需手动复现或参考。

**适用系统**：Debian 10 / 11 / 12 / 13（及后续版本）

---

## 文档目录

| 文件 | 内容 |
|---|---|
| [01-system-tuning.md](01-system-tuning.md) | 系统调优：BBR、内核参数、内存管理、日志轮转、系统精简 |
| [02-security-hardening.md](02-security-hardening.md) | 安全加固：SSH 密钥登录、端口随机化、nftables 防火墙 |
| [03-docker.md](03-docker.md) | Docker Engine 与 Compose 安装与配置 |
| [04-xray.md](04-xray.md) | Xray Core 部署与规则集管理 |
| [05-warp.md](05-warp.md) | 代理生态：Cloudflare WARP 与 Usque (MASQUE) 客户端部署 |
| [06-devops-tools.md](06-devops-tools.md) | 运维工具：Fish Shell、Micro 编辑器、Yazi 文件管理器、Lego 证书管理 |
| [07-port-forwarding.md](07-port-forwarding.md) | Realm 端口转发服务部署 |
| [08-web-services.md](08-web-services.md) | Web 服务：自编译 Caddy (集成 layer4/naive 插件) 与 Ferron Web 服务器 |

---

## 说明

- 所有命令默认以 **root** 用户执行，或通过 `sudo` 提权。
- 文档中的命令均经过测试，可直接复制执行。
- 部分步骤有多个可选方案（如中国大陆网络加速），请根据实际情况选择。
