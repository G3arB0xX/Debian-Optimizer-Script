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
| [03-podman.md](03-podman.md) | Podman Rootless 容器环境安装与配置 |
| [04-xray.md](04-xray.md) | Xray Core 部署与规则集管理 |
| [05-warp.md](05-warp.md) | 代理生态：Cloudflare WARP 与 Usque (MASQUE) 客户端部署 |
| [06-devops-tools.md](06-devops-tools.md) | 运维工具安装与配置：Fish、Micro、Yazi、Lego |
| [07-port-forwarding.md](07-port-forwarding.md) | Realm 端口转发服务部署 |
| [08-web-services.md](08-web-services.md) | Web 服务：自编译 Caddy (集成 layer4/naive 插件) 与 Ferron Web 服务器 |
| [09-terminal-toolchain.md](09-terminal-toolchain.md) | Fish + Yazi + Micro 使用教程（快捷键、协同工作流；需先完成 06 安装） |

---

## 说明

- 所有命令默认以 **root** 用户执行，或通过 `sudo` 提权。
- **配置文件**：文档以「写入路径 + 带注释的原生配置内容」呈现，请用编辑器手动创建或编辑对应文件；长配置完整内容在 `templates/` 目录（含详细注释），与脚本自动化部署一致。
- **操作命令**：安装、权限设置、服务启停等步骤仍为可直接复制的 bash 命令块。
- 部分步骤有多个可选方案（如中国大陆网络加速），请根据实际情况选择。
- **安装 vs 使用**：`06-devops-tools.md` 负责部署；`09-terminal-toolchain.md` 负责 Fish/Yazi/Micro 的日常使用，二者配套阅读。
