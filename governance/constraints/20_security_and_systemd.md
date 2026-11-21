# 20 安全加固、防火墙与 Systemd 约束

[上一层：10 Bash 与模板](10_bash_and_templates.md) | [约束索引](README.md) | [下一层：30 发布与版本管理](30_release_and_vcs.md)

本层规定 SSH 加固、nftables 模块化防火墙、Fail2ban 全公网端口自动防护、Systemd 安全沙盒以及 Rootless Podman 权限隔离红线。

## §1 SSH 基础安全加固与回滚

1. **可选跳过机制**：首次初始化前提供 `[Y/n]` 确认，允许容器或轻量场景跳过 Root/SSH 加固，同时继续执行其余基础优化。
2. **最小权限账户与密钥登录**：自动检测并引导创建具备 `sudo` 权限的非 root 用户，强制配置 Ed25519 密钥认证。
3. **端口重映射与双模式适配**：自动分配 `40000+` 随机高位 SSH 端口，同时适配 `ssh.socket`（Systemd Socket Activation）与传统 `sshd` 服务模式。
4. **连通性预检与秒级回滚**：切换配置后必须经交互式连通性验证；验证失败立即按备份回滚，防止远程失联。验证通过后再禁用 Root 登录、禁用密码登录并锁定 Root 密码。

## §2 nftables 模块化防火墙架构

1. **默认防火墙栈**：统一使用 `nftables` 取代 UFW，遵循 `/etc/nftables.d` 模块化结构（主配置加载 `/etc/nftables.d`，入口 `50-debopti.nft` 加载私有目录 `/etc/nftables.d/debopti`），实现规则原子隔离与平滑迁移。
2. **标准化接口**：各组件放行或回收端口一律通过 `add_fw_rule`（及对应清理接口）操作，屏蔽底层 `nft` 语法细节并保证幂等清理。

## §3 Fail2ban 最佳实践与全公网开放端口自动同步

1. **脱离 `rsyslog` 持续运行**：因低内存优化会清理 `rsyslog`（见 `10 §5`），Fail2ban 必须安装 `python3-systemd` 并配置 `backend = systemd`，直接从 `journald` 读取 `[sshd]` 与内核日志（`_TRANSPORT=kernel`），`[recidive]` 读取 `/var/log/fail2ban.log`，确保无 `rsyslog` 时 100% 正常封禁。
2. **标准化防护基线**：
   - 封禁动作采用 `nftables-allports`（与本项目 `nftables` 架构一致）。
   - 开启 `[sshd]` 激进模式（`mode = aggressive`）、`[recidive]` 累犯长封与渐进式封禁时长（`bantime.increment`）。
   - 所有配置文件均须存于 `templates/security/` 并经 `render_template` 部署（遵循 `10 §2`）。
3. **自动覆盖全部公网开放端口与变更同步**：
   - 通过 `[debopty-public-ports]` jail 与 nftables 泛洪日志规则（`55-debopty-f2b-guard.nft`）自动防护所有对外开放的 TCP/UDP 公网端口。
   - 部署独立同步工具 `/usr/local/bin/debopty-f2b-sync`（不依赖 `/opt/debopty` 运行时路径），解析 `/etc/nftables.conf` 及全部 `/etc/nftables.d/**/*.nft`（排除自身生成的 `55-debopty-f2b-guard.nft`），动态汇总当前全部公网开放端口并无痛重载规则。
   - 同步触发链路须同时覆盖：① 内部防火墙增删端口 API（及各应用模块放行端口时自动触发）；② `systemd.path` 监听 `/etc/nftables.conf` 与 `/etc/nftables.d` 变更；③ `systemd.timer` 周期兜底；④ `nftables.service.d` override 钩子，确保手工或第三方修改端口后自动同步生效。
   - 手动复现与排障步骤同步维护于 [../../docs/02-security-hardening.md](../../docs/02-security-hardening.md)。

## §4 Systemd 安全沙盒与最小权限

1. **强制沙盒指令**：所有由本项目创建或托管的 Systemd 服务单元必须包含 `ProtectSystem=full`、`PrivateTmp=true` 与 `NoNewPrivileges=true`。
2. **第三方服务加固**：对于官方脚本或包管理器自带的第三方单元，不得直接篡改 `/lib/systemd/system/` 原文件，一律通过 `/etc/systemd/system/<unit>.service.d/` drop-in override 注入沙盒与资源限制。
3. **最小权限原则**：严禁无理由以 `root` 运行业务服务；非 root 服务如需监听 `<1024` 低位端口，统一通过 `AmbientCapabilities=CAP_NET_BIND_SERVICE` 授予最小能力。

## §5 Rootless Podman 与证书权限隔离

1. **无守护进程专用用户**：以专用 `apps` 用户运行 rootless Podman，安装前自动卸载 Docker 避免冲突；业务数据放 `/home/apps/srv`，逻辑备份放 `/home/apps/backups`（对齐 Restic 教程）。
2. **存储与 ACL 禁忌**：引擎 graphroot 使用默认 `~/.local/share/containers/storage`，**严禁**对其施加 maintainer ACL 以防破坏 user namespace 映射。
3. **配置与运行时细节**：自动部署 `containers.conf`（`journald` 日志、`systemd` cgroup；`database_backend=sqlite` 仅对 Podman 5.x+ 条件写入，不得写入错误的布尔型 `active_service=true`）、镜像加速 drop-in、Quadlet 目录、`restic-excludes` 与 `podman-compose`；TUI 提供镜像清理定时器开关（默认开启）。
4. **证书组与命令隔离**：安装时将 `apps` 加入 `debopti-certs` 组以只读访问 `/etc/ferron/certs`；`sudoers` 仅授权 `restic`/`bash -c`/`mkdir` 供 `sudo -u apps` 备份；`apppod`/`appctl`/`applog`/`appshell` 快捷命令仅向 SOT 用户与 `root` 暴露（Bash `profile.d` 与 Fish `conf.d` 同步校验），其余用户仅提供 `docker`->`podman` 通用别名。详情见 [../../docs/03-podman.md](../../docs/03-podman.md)。
