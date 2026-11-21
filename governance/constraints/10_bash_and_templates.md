# 10 Bash 规范、模板引擎与终端工具链

[上一层：00 总则](00_general.md) | [约束索引](README.md) | [下一层：20 安全与 Systemd](20_security_and_systemd.md)

本层规定 Pure Bash 编码纪律、模板渲染、自举安装、基础系统调优、终端工具链快捷键标准以及各核心组件的不变量。

## §1 严格 Bash 与防御性编程

1. **严格模式**：所有 Bash 脚本统一启用 `set -euo pipefail`，兼容 Debian 10–13 及更新版本。
2. **防御性写法**：所有变量展开必须用双引号包裹；所有外部网络、包管理与子进程调用必须具备超时控制与错误捕获，防止挂起或静默失败。
3. **可读性与报错**：对关键逻辑与抽象流程编写清晰注释；运行失败时输出明确原因与可操作的修复指引。权限不足时应在不异常退出的前提下引导用户提权。
4. **模块复用与非交互**：通用能力拆分为独立函数供各模块复用，不重复造轮子；全局支持 `CI=true` 非交互模式，自动跳过交互确认以适配无人值守部署。

## §2 模板统一管理与动态渲染

1. **零内联配置 Here-Doc**：所有系统配置、服务单元、防火墙规则与应用配置文件的原始内容必须集中存放在 `templates/` 对应子目录中管理，**严禁**在脚本代码中通过 Here-Doc 硬编码配置正文。
2. **统一渲染引擎**：安装、更新与重配置时一律通过纯 Bash 实现的 `render_template` 引擎读取模板、替换占位符并原子部署至目标路径。

## §3 幂等性、最小侵入与原子还原

1. **最小侵入**：优先采用 `.d` 独立配置目录与 `systemd` drop-in override，减少对系统原厂主配置的直接改写。
2. **幂等与记忆**：重复执行任何安装或调优流程不得产生配置堆叠、报错或破坏既有资产；依托 `/etc/debopti/debopti.conf` 持久化地区、基础优化状态与模块开关，按系统版本自动选择最佳实现。
3. **原子化还原**：支持撤销脚本写入的配置与服务变更，按操作前备份原样恢复系统。

## §4 无 `curl`/`wget` 自举安装与网络底层

1. **零依赖下载降级链**：一键安装命令与 `install.sh` 必须兼容既无 `curl` 也无 `wget` 的最小化 Debian 系统，按 `curl` -> `wget` -> `python3 urllib` -> `perl HTTP::Tiny` -> `apt-get install -y ca-certificates curl` 顺序自动回退，并通过临时文件执行脚本以保持交互式 TTY 可用。
2. **自举迁移与全局命令**：首次运行若不在 `/opt/debopty`，自动迁移安装并创建 `/usr/local/bin/debopty` 软链及 `profile.d/debopty.sh`、`fish/conf.d/debopty.fish`，同时自动迁移清理旧 `debopti` 快捷入口；生产安装目录必须剔除开发元数据（见 `30 §1`）。
3. **运行时 `HOME` 补全**：入口脚本启动时调用 `ensure_runtime_home()`，在容器或非登录 shell 未设置 `HOME` 时对 root 补全 `/root`、普通用户从 `getent passwd` 解析并兜底 `/tmp`，避免 `set -u` 触发未绑定错误。
4. **DNS 自检与镜像池**：启动时通过 `getent hosts` 检测 DNS，异常时先写入 `1.1.1.1` 救活，地区探测（多节点 `ipinfo.io`/`cip.cc` 等）后再按海外（`8.8.8.8`/`1.1.1.1`）或中国大陆（`223.5.5.5`/`119.29.29.29`）二次校准，并按双轨 IPv6 探测结果同步写入 IPv6 DNS；原始文件备份至 `/etc/resolv.conf.debopti.bak`。境内自动启用 APT 与 GitHub 镜像加速，统一经 `download_with_fallback` 与 `git_clone_with_fallback` 轮询下载。

## §5 基础系统调优、默认 `tmux` 与终端快捷键标准

1. **基础系统调优（Base Optimization）**：覆盖 APT HTTPS 与镜像源、Cloud 精简内核检测提示、Sysctl `.d` 网络并发调优、ZRAM（`zstd`）/Swap 交互配置、Logrotate 按天压缩轮转、低配 VPS 瘦身（保留 2 个 TTY、移除 `rsyslog`、限制 Journald 配额、可选屏蔽 ModemManager/Avahi）、`Asia/Shanghai` 时区与 `chrony`、IP 转发一键切换及基础安全加固（见 `20 §1`–`§3`）。
2. **初始化默认安装 `tmux`**：`run_base_optimization` 默认调用 `setup_tmux` 安装 `tmux` 并通过 `templates/system/tmux.conf` 渲染 `/etc/tmux.conf`（兼顾 Debian 10–13 版本语法安全、TrueColor、OSC52/Yazi 图像透传、鼠标支持、大历史缓冲、直观分屏键位与备份/原子还原），同时接入 DevOps 子菜单支持状态展示与 `uninstall_tmux`。
3. **终端工具链快捷键规范（Windows / VS Code / Micro 风格）**：
   - 核心操作：`Ctrl+C/X/V` 复制/剪切/粘贴、`Ctrl+S` 保存、`Ctrl+Q` 退出、`Ctrl+F` 查找/过滤、`Ctrl+G` 跳转行、`F2` 重命名、`Ctrl+N` 新建。
   - 进阶操作（`Alt+*` 避免与 Yazi 冲突）：`Alt+P` 模糊找文件、`Alt+F` 全局搜索、`Alt+J` 屏内跳转、`Alt+[/]` 导航历史。
   - 集成终端：Yazi 在当前目录唤出 Fish 的主键为 ``Ctrl+` ``，`t=>e` 仅作 SSH 终端兜底。
   - 禁止项：不得以依赖 CSI u 或 Shift 区分的按键作唯一绑定；不得回退为 Vim 单键（`hjkl`/`y`/`d`）作默认方案。落地文件为 `templates/apps/devops/yazi_keymap.toml`、`micro_bindings.json`，教程见 [../../docs/09-terminal-toolchain.md](../../docs/09-terminal-toolchain.md)。

## §6 组件模块与 TUI 不变量

1. **Xray Core**：调用官方脚本部署，默认关闭自启与原厂规则；支持官方与 Loyalsoldier 规则集切换，geo 自动更新 Cron 由 `XRAY_GEO_AUTO_UPDATE` 独立持久化并在更新核心时保留。
2. **虚拟组网（Easytier & Tailscale）**：Easytier 以 `/opt/easytier/easytier-core` 判已安装并调用官方 `update`，更新不改 `easytier@` 自启状态，首装默认关闭自启；经 `/etc/profile.d/debopti-easytier.sh` 与 Fish SOT PATH 暴露命令并支持卸载还原，教程见 [../../docs/10-easytier.md](../../docs/10-easytier.md)。
3. **Rust 与 Web/转发生态**：`rustup` 境内配 `rsproxy` 并降权装至非 root 主账号家目录，经 Fish SOT 分发；Realm 多架构二进制配独立用户与 Systemd 沙盒；Ferron 走官方 APT 源配自动 TLS 与 KDL。
4. **WARP / Usque**：支持 WARP Socks5 切换与 Systemd 内存限制、Usque（MASQUE）JWT 注册及 WireGuard Xray 出站生成器（优选 Endpoint、探测极限 MTU）。
5. **Go 与自编译生态**：Go 境内配 `goproxy.cn` 并追踪 `SUDO_USER` 将 `GOPATH` 降权置于主账号家目录；定制 Caddy 用 `xcaddy` 集成 L4/Cloudflare DNS/naiveproxy；Tailscale DERPer 克隆源码经 `sed` 注入防拨测补丁，以 `derper` 用户显式指定 `-c /opt/derper/derper.key` 并输出 ACL JSON。
6. **DevOps 工具栈**：Fish/Micro/Yazi 采用 SOT 只读软链模式（`chmod o+rX` 清 `o+w`，保留各用户 `fish_variables`/`fish_plugins`/`fish_history`/`conf.d.local/` 可变，启动时 `maybe_sync_devops_sot_on_startup` 同步）；共享 CLI（`fzf`≥0.53、`fd`、`ripgrep`、`zoxide`、剪贴板、`bat`、0xProto Nerd Font）统一走 `_install_devops_shared_cli_deps()`；`install_fish` 覆盖 `puffer_fish_expand_dot.fish` 兼容 Debian Fish 3.x；`install_micro` 对 MicroOmni 应用 `micro_omni_session.sed` 修正 `goos.MkdirAll`；Yazi 默认 `sort_by=natural`、`linemode=mtime`、`ratio=[1,3,5]`、图片预览 `10000` 自适应与长行换行，多媒体预览首装 `y/N` 写入 `YAZI_MEDIA_PREVIEW`；Lego v5.2+ 用 `lego run`、`1.1.1.1:53`、`DEBOPTI_DNS_SKIP_PROPAGATION`、`debopti-lego-run-once.sh` 并自动同步证书至 `/etc/ferron/certs`。
7. **FreshIP v4**：多模块探针优先状态机（达标进入 `maintain` 每日最多一次、漂移或送中进入 `simulate` Google 70% / Trust 30%），单双栈独立 timer、每日热数据 OTA 与 IP-Sentinel `map.json` 同步，教程见 [../../docs/11-freship.md](../../docs/11-freship.md)。
8. **TUI 面板**：多级菜单、`get_status` 实时探测、ANSI 绝对坐标渲染、外部调用超时与 `pause` 报错回显。
