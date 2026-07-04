# AI 自动编程指示 VIBE CODER INSTRUCTION

## 一、 致 Coder 的最高指令

角色与职责：你是本项目唯一的高级全栈架构师、开发工程师与 DevOps 专家，开发经历丰富，熟悉 Debian 系统架构与多种编程语言。 用户画像：当前与你交互的用户（产品经理 PM）不懂得写代码，不理解报错。

因此，你需要遵守以下法则：

- 全包揽与零占位符：从环境检查、构建项目目录，到编写代码、排查报错，全部由你通过执行终端命令自动完成，或给出绝对傻瓜化的一键复制指令。严禁在生成的代码中使用 `// ... 在此处添加逻辑` 或 TODO 占位符。你提交的每一次修改必须是逻辑闭环、可编译运行的生产级代码。
- 强制单步计划模式 (Plan Mode)：在开始每一个新功能模块（Sprint）的编码前，必须先输出自然语言的架构设计与执行步骤（Proposal）。当 PM 明确回复“同意/继续”后，方可落盘修改文件。
- 自主查错机制：表写的代码需要你运行测试和排查报错。遇到报错时，自行读取 Terminal 日志或要求 PM 粘贴报错全文，你必须自己分析、定位并修改代码修复它，绝对禁止让 PM 自己去阅读或修改源码。
- 代码连续性：你应该读取目前最新的项目代码，以当前最新代码为基础（而不是你在历史记录里写的版本）修改。修改应当最小化，尽量不改动其他代码，不做无意义的修改和删减使得其他功能缺失，或者使得其他agent添加的功能被删掉，保证代码修改的连续性。
- 语言风格约束：本项目所有文本（TUI、注释、文档、配置）必须保持简洁、清晰、专业。严禁使用过度包装的词汇（如“最终版”、“智能”、“深度”、“极致”等）或非标准词汇。描述功能应精确对齐，不堆砌形容词。
- 文档同步：每次增删功能，你都应该修改本文挡，特别是“项目功能与结构”部分，使本文档的内容与项目最新版本一致。
- 模板统一管理：编辑、渲染或部署任何系统或应用配置文件时，其原始文件内容必须统一作为模板存放在 `templates/` 目录中管理，绝不允许在脚本代码中硬编码（如 Here-Doc 直写）配置文件内容。安装或更新时一律通过 `render_template` 引擎进行动态替换并部署。
- 增量文档更新与手动配置教程：每次新增功能、调整组件或修改配置时，除了修改代码本身，必须酌情考虑在 `docs/` 目录下的对应说明文档中同步增加或更新手动配置与部署教程，保证自动化脚本的操作与手动复现步骤 100% 对齐。
- 交互与交付物语种要求：与用户（PM）的所有日常回复、解释说明，以及实施计划方案（Implementation Plan/Proposal）、执行步骤清单（Task List/task.md）、最终执行报告（Walkthrough/walkthrough.md）等所有交互场景与交付文档，均必须且只能使用中文撰写。



## 二、 项目全局约束与技术栈

### 2.1 基本信息

项目正式名称/商标：Debian Optimizer Script
作者 GitHub 账号：G3arB0xX
远程仓库地址：https://github.com/G3arB0xX/Debian-Optimizer-Script
项目内容：这是一个开箱即用的 TUI 脚本，用于 Debian 系统的环境初始化、系统性能调优以及若干服务自动化的部署与管理。
目标平台：首发 Debian 10，兼容 Debian 10~13 及以后最新的 Debian 版本。
核心语言：Bash

### 2.2 技术栈红线与架构纪律

- 高安全性与健壮性：确保脚本安全、可靠。遇到最坏情况不会崩溃或出现严重漏洞，不会运行高危命令，绝不破坏系统运行与用户资产。
- 最优化性能：优化代码执行逻辑，实现最佳运行性能与最快运行速度。
- 标准化方法：脚本应使用标准的、现代的、最受认可的、当下最佳实践的方法实现其功能。
- 最小化修改：最大程度降低对系统的侵入性。比如，使用 `.d` 目录与 systemd edit 等现代的配置文件修改方法。
- 原子化还原：可撤销脚本执行的操作，完全按操作前的原样还原系统。
- 幂等性设计：绝不因重复执行导致配置堆叠、报错或文件被破坏。
- 防重复执行：对只需要执行一次的命令与保留执行记忆。记忆系统状态与脚本状态，以在下次运行脚本时智能识别环境。
- 强兼容性：智能识别系统版本，使用符合相应系统版本的最佳方法实现相关功能。
- 模块复用：各种功能拆分为简单的函数，使用时调用，不重复造轮子。
- 详细报错：脚本运行失败后必须有详细报错以及修复指导。如果脚本运行权限不够，应在不关闭脚本的情况下指导用户智能提权。
- 详尽注释：详细注释重要代码与抽象流程，增加代码可读性，提高可维护性。
- Systemd 安全沙盒化 (Sandboxing)：所有服务单元必须包含 `ProtectSystem=full`, `PrivateTmp=true`, `NoNewPrivileges=true`。对于第三方服务，使用 `.service.d` 目录进行 override 注入。
- 最小权限运行 (Privilege Minimization)：严禁无理由以 root 运行业务服务。对于需要绑定低位端口的非 root 服务，使用 `AmbientCapabilities=CAP_NET_BIND_SERVICE`。
- 防御性 Shell 编程：所有变量必须被双引号包裹，所有外部调用必须有超时控制与错误捕获。
- **终端工具链快捷键规范 (Windows / VS Code / Micro 风格)**：Fish + Yazi + Micro 的自定义快捷键必须对齐 **Windows 图形界面、VS Code 与 Micro 编辑器**的共同习惯，降低从桌面环境迁移到 SSH 终端的认知成本，三工具之间的键位应相互一致、可预期。
  - **核心操作 (`Ctrl+*`)**：与 Windows 资源管理器、VS Code 与 Micro 保持一致——`Ctrl+C/X/V` 复制/剪切/粘贴，`Ctrl+S` 保存，`Ctrl+Q` 退出，`Ctrl+F` 查找或过滤，`Ctrl+G` 跳转行，`F2` 重命名，`Ctrl+N` 新建。
  - **进阶功能 (`Alt+*`)**：Micro 插件映射 VS Code 同类能力，且避免与 Yazi 已占用的 `Ctrl+*` 冲突——`Alt+P` 对应 VS Code `Ctrl+P`（模糊找文件），`Alt+F` 对应 `Ctrl+Shift+F`（全局搜索），`Alt+J` 屏幕内跳转，`Alt+[/]` 导航历史。
  - **集成终端**：Yazi 在当前目录打开 Fish 终端的主键为 **`Ctrl+\``**（VS Code 集成终端同款）；`t⇒e` 两步组合键仅作 SSH 终端兜底。
  - **禁止项**：不得将依赖 CSI u 或 Shift 区分的按键（如 `!` = Shift+1）作为唯一绑定；不得回退为 Vim 单键（`hjkl` 导航、`y`/`d` 删改）作为默认方案。
  - **落地文件**：`templates/apps/devops/yazi_keymap.toml`、`micro_bindings.json`；使用教程见 `docs/09-terminal-toolchain.md`。

### 2.3 环境隔离与版本管理

- **开发环境 (Dev)**：本项目使用 `jujutsu (jj)` 进行版本管理。作为 Coder，你需要提示 PM 使用 `jj` 提交和推送修改，以维持开发流。
- **生产环境 (Prod)**：脚本分发至用户端后，属于生产运行态。**严禁在生产代码、TUI 界面、日志输出或任何用户交互中提及 `jj`、`jujutsu` 或“版本提交”等开发术语。** 
- **资产同步**：在安装/同步脚本资产时，必须确保排除 `.git`、`.jj`、`.github` 等开发相关目录，保持生产环境的纯净与轻量。
- **版本标识 (Version Branding)**：每次使用 `jj change` 提交修改后，必须将当前 Change ID 写入 `scripts/common.sh` 中的 `VERSION_ID` 变量。该 ID 需在 TUI 主界面显示，用于生产环境下的版本追溯。
- **测试脚本统一管理**：所有开发期测试脚本（模块冒烟、交互验证、临时探测等）必须统一放入 `tests/` 目录并按模块分子目录管理（如 `tests/podman-test-base/`、`tests/yazi/`）。禁止在仓库根目录或已废弃的 `scratch/` 等路径散落测试文件。
- **GitHub 脱敏推送**：本地开发完成后，使用 `instructions/github-push.sh` 将代码推送至 GitHub 公开仓库。该脚本在隔离沙盒中重写 Git 历史，自动剥离 `instructions/`、`tests/` 及所有 vibeinstruction 变体，本地 jj 工作区不受影响。PM 要求推送时 Coder 按 §2.4.1 固定流程执行。

### 2.4 GitHub 脱敏推送流程

开发仓库（jj 管理）与 GitHub 公开仓库内容有意分离：公开仓库不包含内部指令文档与测试脚本。

**脚本入口**：`instructions/github-push.sh`（仓库根目录执行）

**远程目标**：`git@github.com:G3arB0xX/Debian-Optimizer-Script.git`，分支 `main`

#### 2.4.1 AI Coder 调用协议（固定流程）

当 PM 说「脱敏推送」「推送到 GitHub」「github push」「执行 github-push」等同类意图时，Coder **必须**按下列顺序自动执行，**禁止**使用 `jj git push`、`git push origin` 等直接推送（会把 `instructions/`、`tests/` 等敏感内容原样推上 GitHub）。

**阶段 A — 推送前准备（Coder 自动完成）**

1. **确认工作区已落盘**：代码、文档（`docs/`）、模板（`templates/`）修改已写入磁盘。

2. **总结并撰写 describe**：自上一 change（`@-`）起汇总变更，参考以往 `feat:` / `fix:` / `docs:` 格式撰写完整 description（首行摘要 + 英文 bullet），写入当前待推送 change：
   ```bash
   jj describe -m "$(cat <<'EOF'
   feat: <one-line summary>

     - <bullet 1>
     - <bullet 2>
   EOF
   )"
   ```
   若工作区仍有未落入 change 的修改（`jj diff --stat` 非空），先折叠进待推送 change（见步骤 4），再对**含代码的 change** 执行 describe。

3. **确定推送目标 change（`PUSH_REV`）**：
   - 若 `@` 为 **empty** change（`jj log -r @` 显示 `(empty)`），则 `PUSH_REV=@-`（父 change 承载本次功能）。
   - 否则 `PUSH_REV=@`。
   ```bash
   PUSH_REV='@'
   if jj log -r @ -T 'if(empty, "empty", "nonempty")' 2>/dev/null | grep -qx empty; then
       PUSH_REV='@-'
   fi
   ```

4. **折叠工作区修改**（`jj diff --stat` 非空时执行）：
   ```bash
   JJ_EDITOR=true jj commit docs/ scripts/ templates/ tests/ instructions/ README.md README_CN.md deb_optimizer.sh install.sh
   ```
   执行后 `@` 变为新的 empty change，原修改落入 `@-`；此时将 `PUSH_REV` 设为 `@-`。

5. **版本标识**：将 `scripts/common.sh` 中 `VERSION_ID` 更新为 **`PUSH_REV` 的 Change ID**（非 empty 子 change 的 ID）：
   ```bash
   CHANGE_ID=$(jj log -r "${PUSH_REV}" -T 'change_id.short()' --no-graph)
   # 编辑 scripts/common.sh: VERSION_ID="<CHANGE_ID>"
   ```
   若步骤 4 刚执行过，须再次 `JJ_EDITOR=true jj commit scripts/common.sh` 将 VERSION_ID 落入 `PUSH_REV`，或于步骤 4 前预先改好。

6. **测试目录**：新增测试脚本必须在 `tests/` 对应子目录。

7. **同步 Git `main` 书签**（`github-push.sh` 沙盒克隆检出的是 Git `main`，不是 jj `@`）：
   ```bash
   jj bookmark set main -r "${PUSH_REV}" --allow-backwards
   jj git export
   git checkout main
   ```
   验证：`git rev-parse main` 与 `jj log -r "${PUSH_REV}" -n 1 --no-graph` 提交一致。

**阶段 B — 执行脱敏推送**

```bash
bash instructions/github-push.sh
```

可选强制重写：

```bash
bash instructions/github-push.sh --force
```

**阶段 C — SSH 密钥密码（人机协作）**

脚本按以下优先级加载 GitHub SSH 私钥，**全程只需输入一次**：

| 优先级 | 场景 | 做法 |
|--------|------|------|
| 1 | 密钥已在 ssh-agent | 自动跳过 |
| 2 | 已配置 systemd 用户凭证 | 自动从 `~/.config/credstore/debopti-github-ssh.cred` 解密（见下方一次性配置） |
| 3 | 本地终端有 `/dev/tty` | 脚本提示，用户在终端输入 passphrase |
| 4 | **Cursor / AI 对话框**（无 TTY、无凭证） | PM 在对话中提供 passphrase；Coder 执行 `SSH_PASSPHRASE='<密码>' bash instructions/github-push.sh` |
| 5 | 管道传入 | `printf '%s\n' '<密码>' \| bash instructions/github-push.sh --passphrase-stdin` |

**一次性配置 systemd 凭证**（推荐，Cursor 脱敏推送无需再在对话中传密码）：

```bash
bash instructions/github-ssh-cred-setup.sh
# 或非交互: printf '%s\n' '<密码>' | bash instructions/github-ssh-cred-setup.sh --passphrase-stdin
```

凭证经 `systemd-creds --user` 加密，仅当前用户可解密，**勿提交** `.cred` 文件至 jj/Git。

Cursor 环境推荐流程：

1. 若尚未配置凭证，PM 本地执行一次 `github-ssh-cred-setup.sh`（或 Coder 用 `--passphrase-stdin` 写入）。
2. Coder 直接运行 `bash instructions/github-push.sh`；脚本自动从 systemd 凭证加载密钥。
3. 若凭证不存在且无 TTY，脚本打印 `DEBOPTI_SSH_PASSPHRASE_REQUIRED`，PM 可在下一条消息提供密码或先完成凭证配置。

若报「未找到 GitHub SSH 私钥」，提示 PM 在 `~/.ssh/config` 为 `github.com` 配置 `IdentityFile`。

**阶段 D — 推送后验证（Coder 自动完成）**

1. 脚本输出「脱敏推送成功完成」且退出码为 0。
2. 抽查 GitHub：
   ```bash
   curl -sL "https://raw.githubusercontent.com/G3arB0xX/Debian-Optimizer-Script/main/scripts/common.sh" | grep VERSION_ID=
   ```
   确认与本地 `PUSH_REV` 的 Change ID 一致。
3. 确认 GitHub 仓库树中**不存在** `instructions/`、`tests/`。

**阶段 E — 收尾与汇报**

1. **新建工作 change**（若 `@` 已是 empty 则跳过）：
   ```bash
   jj new
   ```
2. 用中文向 PM 汇报：是否跳过、是否成功、`VERSION_ID`、是否使用 `--force`、当前 `@` change 状态。失败时附报错与修复建议。

**阶段 A–E 一键检查清单（Coder 内部）**

```
[ ] jj describe 已写入（PUSH_REV）
[ ] VERSION_ID == PUSH_REV 的 change_id
[ ] jj bookmark set main -r PUSH_REV && jj git export && git checkout main
[ ] github-push.sh 成功（或 SSH_PASSPHRASE 重试后成功）
[ ] curl 验证 GitHub VERSION_ID
[ ] jj new（若需要）
```

#### 2.4.2 脚本行为摘要

| 步骤 | 行为 |
|------|------|
| 智能跳过 | `git fetch origin main` 后若本地 `main` 与 `origin/main` hash 相同且未传 `--force`，直接退出，不重复推送 |
| 隔离沙盒 | 在 `.cache/push-temp/` 创建 `--no-hardlinks` 阴影克隆，本地 jj 工作区与 `.jj/` **不受影响** |
| 历史净化 | `git filter-branch` 从全部 commit 的索引中移除脱敏路径（保留 commit 结构，仅剔除文件） |
| 安全强推 | `git push github main --force-with-lease`（非裸 `--force`） |
| 清理 | `trap` 自动删除沙盒目录与 SSH ControlPath 套接字 |

**脱敏范围（从 GitHub 历史中移除）**

| 路径 | 说明 |
|------|------|
| `instructions/` | 含 VIBEINSTRUCTION.md、`github-push.sh` 及内部运维文档 |
| `tests/` | 含 Podman 模块测试、Yazi 交互验证等全部测试脚本 |
| 根目录 vibeinstruction 变体 | `vibeinstruction.md`、`vibeinstruct.md`、`VIBEINSTRCTION.md`、`VIBEINSTRUCT.md`、`VIBEINSTRCT.md` |

**禁止事项**

- **禁止** `git push origin` / `jj git push` 直达 GitHub（绕过脱敏）。
- **禁止** 手动删除本地 `instructions/` 或 `tests/` 后再推送（破坏本地开发资产；脱敏仅应在沙盒副本中进行）。
- **禁止** 在 GitHub 公开仓库中恢复或提交上述脱敏路径。

**可选配置**（`instructions/github-push.sh` 顶部与参数）

- `ENABLE_GPG_SIGNING=false`（默认）：脱敏后 commit 不重新 GPG 签名。若改为 `true`，需 gpg-agent 已缓存密码，否则 filter-branch 会对每个 commit 反复提示。
- `--force` / `-f`：本地与远程 hash 相同时仍强制脱敏重写并推送。
- `--passphrase-stdin`：从标准输入读取一行 SSH 密钥密码（可配合 `printf '%s\n' '密码' | bash instructions/github-push.sh --passphrase-stdin`）。
- 环境变量 `SSH_PASSPHRASE`：供 Cursor/AI 对话框模式，由 Coder 在 PM 提供密码后注入（**勿写入仓库或 commit**）。
- `instructions/github-ssh-cred-setup.sh`：将 SSH 密码写入 `~/.config/credstore/debopti-github-ssh.cred`（systemd 用户加密凭证），供 `github-push.sh` 自动加载。

## 三、项目功能与结构

### 3.1 底层架构与核心机制

* **全局网络环境自举与防封锁网络池**：脚本启动时通过多节点（ipinfo.io, cip.cc 等）侦测服务器归属地。若位于中国大陆，则全局启用 APT 与 GitHub 镜像加速。
* **DNS 健康检查与自动修复**：脚本启动时通过 `getent hosts`（libc6 内置，无需 curl 即可运行）检测 DNS 可用性。若检测到 DNS 故障，先写入 `1.1.1.1` 临时救活（确保后续 apt 安装 curl 成功），再在地区探测完成后执行二次校准：海外写入 `8.8.8.8/1.1.1.1`，境内写入 `223.5.5.5/119.29.29.29`；若机器具备 IPv6 公网访问能力（双轨检测：本地 global 网卡地址 + 外部 API 回退），同步写入对应的 IPv6 公共 DNS。DNS 正常时跳过，不修改用户配置。原始配置自动备份至 `/etc/resolv.conf.debopti.bak`。
* **高可用下载与克隆模块**：封装 `download_with_fallback` 与 `git_clone_with_fallback`，内置混合模式镜像池轮询机制（涵盖 CDN、教育网、高优代理站等），具备严格超时控制与底层断流防挂起策略。
* **持久化状态管理**：依托 `/etc/servopti.conf` 文件记录系统状态（如网络归属地 `IS_CN_REGION`、基础优化完成标记 `BASE_OPTIMIZED`），实现断点记忆与防重复执行。
* **安全防御与权限管控**：默认使用 nftables 代替 UFW，采用 **`/etc/nftables.d` 模块化架构**（主配置加载根目录，模块通过 `50-debopti.nft` 入口文件加载私有目录），实现规则的原子化隔离与平滑迁移。
* **集中式模板管理与动态渲染**：将系统、安全、组件应用等所有配置文件的原始硬编码（Here-Doc）剥离至根目录的 `templates/` 文件夹中并按子目录归类整理。封装纯 Bash 实现的 `render_template` 引擎在安装与配置部署时动态读取模板，安全替换占位符（如地区标记、运行端口、环境目录等）并渲染至系统目标位置，大幅提升系统可维护性与配置透明度。
* **自动化自举安装与全局命令绑定**：脚本首次运行时自动检测环境，若非位于 `/opt/debopti` 则自动迁移安装，并在系统路径创建软链接 `debopti`，支持全局命令行唤起。
* **运行时 HOME 补全**：`ensure_runtime_home()` 在入口脚本启动时检测 `HOME`；容器或非登录 shell（如 `bash -c` 管道安装）未设置时，root 补为 `/root`，普通用户从 `getent passwd` 解析，兜底 `/tmp`。避免 `set -u` 下 `$HOME` 未绑定导致启动失败。
* **模块化解耦架构**：代码按功能拆分为 `scripts/*.sh` 与 `scripts/apps/*.sh`，通过主入口动态加载。实现高内聚低耦合，方便后续组件化扩展。
* **运维与终端增强工具栈**：集成 Fish Shell、Micro 编辑器与 Acme.sh 证书管理，提供从 Shell 到编辑器再到安全证书的全方位运维环境自动化构建。
* **标准化防火墙接口**：封装 `add_fw_rule` 接口，屏蔽底层 nftables 语法细节，支持模块化规则的自动化下发与幂等化清理，确保系统安全且易于维护。

### 3.2 基础系统调优模块 (Base Optimization)

涵盖服务器初始化与网络底层的全方位优化，首次运行强制执行，支持随时重复调用而不产生副作用。

* **包管理器与基础依赖**：智能替换镜像源，强制升级 APT HTTPS 传输，自动补齐基础系统组件。
* **内核与网络协议栈 (Sysctl)**：自动检测并提示更换 Cloud 专用精简内核；通过写入 `.d` 独立配置文件优化网络并发。
* **内存与日志控制**：提供 ZRAM（zstd 算法压缩）与 Swap 交换文件的交互式配置；重写 Logrotate 规则，实现系统日志按天压缩轮转，防磁盘爆满。
* **内存极限瘦身 (Extreme Slimming)**：针对低配 VPS 强制削减多余 TTY 终端（保留 2 个），移除冗余日志系统 (rsyslog)，严格限制 Journald 配额，并支持交互式屏蔽 ModemManager/Avahi 等冗余服务。
* **基础安全加固 (SSH Hardening)**：
    *   **可选跳过机制**：首次运行前提供交互式 `[Y/n]` 确认选项，允许在容器、轻量化或不需要加固 Root/SSH 的环境下一键跳过加固流程，无缝执行其余基础系统调优。
    *   **非 Root 用户管控**：自动检测并引导创建具备 sudo 权限的普通用户，强制脱离 root 直接登录。
    *   **密钥强制验证**：强制配置 Ed25519 密钥登录，本地生成密钥对，云端自动化注入。
    *   **端口随机重映射**：自动分配 40000+ 随机高位端口；深度适配 Systemd Socket (ssh.socket) 与传统 sshd 模式。
    *   **连通性预检与回滚**：内置交互式连通性验证逻辑，验证失败自动秒级回滚，防止远程失联。
    *   **终极封锁**：验证成功后自动禁用 Root 登录、禁用密码登录、锁定 Root 密码。
*   **时区自动校准**：配置系统时区为 Asia/Shanghai 并部署 chrony 服务保障时间同步。
* **路由转发控制**：提供独立的 IP 转发（IP Forwarding）状态切换功能，支持在纯建站模式（关闭转发）与代理/组网/容器模式（开启转发）间一键切换。

### 3.3 自动化组件部署模块

基于 TUI 提供常用网络、代理与开发组件的安装、更新、卸载完整生命周期管理。具备运行状态记忆与平滑重启能力。

* **Xray Core**：调用官方脚本部署（默认关闭开机自启且默认使用官方原厂规则）。提供专有节点管理菜单，支持在官方默认规则集与 Loyalsoldier 增强规则集之间一键切换；geo 规则自动更新（Cron）与规则集独立，由 `XRAY_GEO_AUTO_UPDATE` 持久化，更新 Xray Core 时保持用户开关状态。

* **虚拟组网工具 (Easytier & Tailscale)**：支持跨平台组网客户端的智能拉取、提权安装与防火墙 P2P 端口自动放行。Easytier 以 `/opt/easytier/easytier-core` 判定已安装并调用官方 `update`（非 `install`）；更新时不改动 `easytier@` 自启状态，由官方脚本按更新前实例保持运行/停止。首次安装后默认关闭 `easytier` / `easytier@` 开机自启；`easytier-cli`、`easytier-core` 经 `/etc/profile.d/debopti-easytier.sh` 与 Fish SOT PATH 加入环境变量，卸载时还原。手动步骤见 `docs/10-easytier.md`。

* **Rust 环境与中转生态**：
  * **Rust 运行环境 (rustup)**：一键安装与修复，国内自动配置字节跳动 (rsproxy) 极速镜像。自动识别非 root 主账号并降权安全部署至其家目录下（如 `/home/testuser/.cargo` 与 `/home/testuser/.rustup`），环境路径通过 Fish SOT 自动向全系统账户分发；卸载时彻底移除以保障原子化还原。
  * **Realm 转发服务器**：基于 Rust 的高性能中转工具。实现多架构二进制自动匹配、Systemd 守护进程自动化配置及独立运行用户安全加固。
  * **Ferron Web 服务器**：极速、轻量级的 Rust Web 服务器。集成官方 APT 仓库部署，支持自动 TLS、KDL 现代语法配置及 Systemd 深度集成。

* **WARP & Usque 代理生态**：
  * 提供 Cloudflare WARP 客户端部署、Socks5 模式切换及 Systemd 内存级限制优化。
  * 内置 Usque (MASQUE 协议) 轻量客户端下载与 JWT Token 自动化注册。
  * 自带 WireGuard 协议的 Xray 出站 JSON 生成器，支持自动抓取账户信息、优选 Endpoint IP、精确探测链路极限 MTU 并计算防封锁特征码。

* **Podman 容器环境 (Rootless)**：以专用 `apps` 用户运行 rootless Podman，无守护进程。自动配置 `containers.conf`（journald 日志、systemd cgroup、sqlite 元数据后端）、镜像加速 drop-in、Quadlet 服务目录与 `podman-compose`。`apppod`/`appctl`/`applog`/`appshell` 仅对 **SOT 用户与 root** 暴露（Bash `profile.d` 与 Fish `conf.d` 均做用户校验/同步排除）；其他用户仅获得 `docker`→`podman` 等通用别名。安装前自动卸载 Docker 以避免冲突；TUI 提供镜像清理定时器开关（默认开启）。

* **GoLang 与自编译应用生态**：
  * **Go 运行环境**：提供 Go 环境一键修复/安装（国内自动配置 `goproxy.cn` 代理）。自动追踪最初运行 `debopti` 的非 root 主账号（`SUDO_USER`），将用户的 `GOPATH` 与编译工作区降权安置在主账号家目录下（例如 `/home/testuser/go`），拒绝污染 `root` ；卸载时彻底移除并自灭状态灯。
  * **定制化 Caddy**：使用 `xcaddy` 实时编译集成 L4、Cloudflare DNS 与 naiveproxy 插件的 Caddy 服务器，并配置标准化 Systemd 守护进程与独立运行用户。
  * **Tailscale DERPer 隐身节点**：实时克隆指定版本源码，通过 `sed` 命令动态注入防拨测补丁（阻断非法域名访问与 /generate\_204 探针），以 `derper` 系统用户运行并显式指定 `-c /opt/derper/derper.key` 节点密钥路径；TLS 证书由 derper 首次启动时按 `-hostname` 自动生成（含 IP SAN），并输出适配 Tailscale 控制台的 ACL 配置 JSON。

* **运维与终端增强工具 (DevOps Tools)**：
  * **Fish / Micro / Yazi 统一配置同步 (SOT 只读软链接模式)**：死配置落在 SOT 用户家目录，其他用户只读共享（`chmod o+rX`，清除 `o+w`）。Fish 不经中间层：非 SOT 用户对 SOT `conf.d` 托管文件逐文件软链，`functions`/`completions` 经 `fish_function_path` 只读引用；`fish_variables`、`fish_plugins`、`~/.local/share/fish/fish_history` 与各用户 `conf.d.local/` 保持本地可变。`debopti` 启动时 `maybe_sync_devops_sot_on_startup` 同步新用户软链。`install_fish` 在 fisher 安装 puffer-fish 后覆盖 `functions/_puffer_fish_expand_dot.fish`（模板 `puffer_fish_expand_dot.fish`），兼容 Debian apt 自带的 Fish 3.x（无 `commandline --search-field`）。
  * **DevOps 共享 CLI 依赖**：Fish / Micro / Yazi 重合工具（`fzf`≥0.53、`fd`、`ripgrep`、`zoxide`、剪贴板、`bat`、0xProto Nerd Font NF/Mono/Propo）统一经 `_install_devops_shared_cli_deps()` 安装，apt 优先、官方 Release/脚本回退，各模块禁止重复实现。
  * **Micro 编辑器与 MicroOmni 插件**：`install_micro` 克隆 MicroOmni 后应用 `templates/apps/devops/micro_omni_session.sed`，将 `Session.lua` 中误写的 `os.MkdirAll` 改为 `goos.MkdirAll`，避免开启 `MicroOmni.AutoSaveEnabled` 时会话自动保存定时报错。
  * **Yazi 文件管理器 (Windows / VS Code / Micro 风格快捷键)**：覆盖 Yazi 默认 Vim 键位，使文件操作对齐 Windows 资源管理器与 VS Code 习惯（`Ctrl+C/X/V` 文件复制剪切粘贴、`F2` 重命名、`Ctrl+Q` 退出等），在当前目录打开 Fish 终端对齐 VS Code **`Ctrl+\``**。默认排序为文件夹置顶 + 自然数升序（`sort_by = natural`）；列表行默认显示修改时间（`linemode = mtime`）；三栏比例 `ratio = [1, 3, 5]`；图片预览 `max_width`/`max_height = 10000` 以随终端自适应；文本预览启用长行换行。可选 `init.lua` 提供 `perm_owner` 组合 linemode。`install_yazi` 调用共享依赖层并补齐 Yazi 专属包（`jq`/`p7zip-full` 等）；多媒体预览（`ffmpeg`/`poppler`/`resvg`/`ImageMagick`）首次 y/N 整组选装，写入 `YAZI_MEDIA_PREVIEW`。键位规范详见 §2.2；配置模板 `yazi_keymap.toml` / `yazi.toml` / `yazi_init.lua`，安装教程 `docs/06-devops-tools.md` §3.1，使用教程 `docs/09-terminal-toolchain.md`。
  * **Lego (Go 语言 ACME 客户端)**：高优获取 GitHub Releases 最新的单二进制包部署于系统全局路径下，无多余环境依赖，卸载时彻底移除二进制文件，保证系统整洁。


* **FreshIP (IP 养护) v4**：对齐 IP-Sentinel 核心养护能力的多模块引擎（`freship_runner` / `freship_probe` / `freship_mod_google` / `freship_mod_trust` / `freship_updater`）。探针优先状态机：国家级三核自检达标后进入 **maintain**（仅探针、不模拟流量，**每日最多一次**），漂移或送中则 **simulate**（Google 70% / Trust 30% 多步会话）。支持 Cookie 持久化、IP 哈希 Persona、region JSON 坐标与 `lang_params`、平台行为矩阵、Referer 域隔离、curl-impersonate 模拟浏览；单栈/双栈独立 timer 与状态；每日热数据 OTA（keywords/region/map/UA）；TUI 同步 IP-Sentinel `map.json`。手册见 `docs/11-freship.md`。

* **自动化 CI 与非交互支持**: 全局支持 `CI=true` 环境变量，在无人值守环境下自动跳过交互确认，实现用户创建、内核更换及系统优化的全自动流水线部署。

### 3.4 交互式 TUI 面板系统

* **多级菜单流转**：构建主菜单与子菜单层级，结构清晰。
* **实时状态感知**：内置 `get_status` 检测函数，精确识别环境变量及特殊装载目录（如 `/opt/`）下的二进制文件，在菜单入口动态渲染各项服务的安装状态标签（\[已安装]/\[未安装]）。
* **绝对坐标渲染引擎**：基于 ANSI 转义序列实现 TUI 窗口的精确绝对定位，适配多种终端尺寸，确保在不同 SSH 客户端下的布局一致性与交互流畅度。
* **异常捕获与容错**：所有可能阻塞或失败的外部调用均配备超时拦截与报错回显，并配合 `pause` 函数中断等待用户阅览日志。系统路径清理采用暴力扫荡确保环境彻底纯净。

### 3.5 模块容器测试（Dev）

在开发机上用 **Podman + Debian 官方镜像** 做模块级冒烟测试，无需真实 VPS。详细参数见 `tests/README.md`。

**测试目录约定**：所有测试脚本统一置于 `tests/` 下，按模块分子目录；GitHub 公开仓库推送时会整体脱敏移除 `tests/`，仅保留于本地 jj 开发仓库。

#### 3.5.1 目录与文件

| 路径 | 用途 |
|------|------|
| `tests/podman-module-test.sh` | Podman 模块测试入口（耗时统计、并行调度） |
| `tests/podman-test-inner.sh` | 容器内验证逻辑（安装、断言、可选重装/卸载） |
| `tests/podman-test-base/Dockerfile` | 预构建基础镜像定义 |
| `tests/podman-test-base/build-base.sh` | 构建 `localhost/debopti-podman-test-base:<tag>` |
| `tests/yazi/` | Yazi 终端文件管理器交互与快捷键验证脚本（tmux + 截图） |
| `.cache/apt/<tag>/archives/` | APT 包缓存（已加入 `.gitignore`） |

#### 3.5.2 常用命令

```bash
# 首次：构建基础镜像（每版本约 20–30 秒，仅需一次）
bash tests/podman-test-base/build-base.sh

# 日常冒烟（默认 fast + bookworm，约 1–2 分钟；网络慢时可能更长）
bash tests/podman-module-test.sh

# 完整集成（含幂等重装与卸载，约 3–5 分钟/版本）
PODMAN_TEST_MODE=full bash tests/podman-module-test.sh

# 指定版本 / 并行
DEBIAN_TEST_TAGS="bookworm bullseye" bash tests/podman-module-test.sh
PODMAN_TEST_PARALLEL=1 PODMAN_TEST_MODE=full DEBIAN_TEST_TAGS="bookworm bullseye" bash tests/podman-module-test.sh

# 跳过基础镜像重建（镜像已存在时）
PODMAN_TEST_SKIP_BUILD=1 bash tests/podman-module-test.sh
```

#### 3.5.3 环境变量

| 变量 | 默认值 | 说明 |
|------|--------|------|
| `PODMAN_TEST_MODE` | `fast` | `fast`：安装 + 验证；`full`：额外测重装/卸载 |
| `DEBIAN_TEST_TAGS` | `bookworm`（fast）/ `bookworm bullseye`（full） | 测试的 Debian 版本 |
| `PODMAN_TEST_PARALLEL` | `0` | `1` 时多版本并行 |
| `PODMAN_TEST_SKIP_BUILD` | `0` | `1` 跳过基础镜像构建 |
| `PODMAN_TEST_FORCE_REBUILD` | `0` | `1` 强制重建基础镜像 |
| `PODMAN_TEST_KEEP` | `0` | `1` 测试后保留容器便于排查 |

#### 3.5.4 加速手段

* **预构建基础镜像**：预装 systemd/sudo/acl/curl，跳过每次重复 apt。
* **APT archives 缓存**：二次运行复用已下载 deb。
* **fast 模式**：跳过重装/卸载，缩短日常开发反馈周期。
* **并行容器**：多 Debian 版本独立跑，缩短 CI 总墙钟时间。

#### 3.5.5 嵌套容器限制（与实机差异）

嵌套 Podman 容器内以下能力**无法完整验证**，测试脚本会自动降级并输出跳过标记，**不代表真实 Debian 主机故障**：

* rootless `newuidmap`（输出 `ROOTLESS_RUNTIME_LIMITED_IN_NESTED_CONTAINER`）
* `systemctl --user` 与 GC 定时器（输出 `GC_TIMER_SKIPPED_NO_USER_SYSTEMD`）
* 主用户 `sudo apppod`（输出 `SUDO_APPPOD_SKIPPED_IN_NESTED_CONTAINER`）

发布前应在**真实 Debian 主机**上补测：`apppod ps`、`appctl status`、`applog -u <service>`。

#### 3.5.6 Podman 模块测试状态（截至当前版本）

| 项目 | 状态 |
|------|------|
| Debian 12 (bookworm) 容器 fast 测试 | **已通过**（安装、配置模板、sudoers、profile.d、apps 用户） |
| Debian 11 (bullseye) 容器测试 | 受 apt 网络波动影响，结果不稳定 |
| 真实 VPS 实机测试 | **未完成**，发布前需补测 |
| 模块代码修复项 | `active_service` 按 Podman 版本条件写入；`apt-get update` 后再探测可选包；systemd 会话不可用时安装不中断 |

Coder 修改 `scripts/apps/podman.sh` 或相关模板后，应至少执行一次 `bash tests/podman-module-test.sh` 确认无回归。
