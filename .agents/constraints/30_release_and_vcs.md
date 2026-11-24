# 30 发布流程、版本管理与脱敏推送

[上一层：20 安全与 Systemd](20_security_and_systemd.md) | [约束索引](README.md) | [下一层：40 测试与 CI](40_testing_and_ci.md)

本层规定 `jj` + `git` 开发流、生产纯净性、**SemVer 2.0.0 + 向后兼容 CalVer/Build** 双轨版本体系、Conventional Commits 1.0.0、Keep a Changelog 1.1.0、Agent 自主发布 SOP 以及 `tools/github-push.sh` 脱敏推送协议。

## §1 开发流与生产环境纯净性

1. **双态隔离**：本地开发仓库由 `jujutsu (jj)` 与 `git` 共同管理；分发至用户 VPS 的运行态属于生产环境。
2. **生产术语禁令**：**严禁**在生产代码、TUI 界面、日志输出或面向终端用户的文案中出现 `jj`、`jujutsu`、Change ID、「版本提交」等开发期术语；对外统一称为 Version / Build。
3. **生产资产净化**：`self_install`、`install.sh` 与发布归档同步/打包时必须剥离 `.git`、`.jj`、`.github`、`tools`、`.agents`（及历史 `governance`）、`tests`、全部 `**/INDEX.md`、`Makefile`、`.shellcheckrc`、`.editorconfig`、根目录 Agent 入口文件及开发期编辑器临时目录，确保生产目录纯净轻量（运行时 VPS Agent 工作环境独立存放于 `/var/lib/debopty/agent`）。
4. **路径限定原子提交**：多代理或并行分片开发场景下，合入与提交须先通过独立只读即时审计，再按已验收分片显式限定提交路径，严禁盲目全量暂存或混入未验分片的脏改动（详见 `80 §2`）。

## §2 SemVer 2.0.0 + 向后兼容 CalVer/Build 双轨身份

1. **主版本规范（SemVer 2.0.0）**：公开版本号由 Agent 自主按 [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html) 管理，格式为 `X.Y.Z` 或预发布 `X.Y.Z-rc.N`，对应干净的 Git annotated tag `vX.Y.Z`（不含 build 后缀）。
2. **向后兼容磁盘键**：根目录 `VERSION` 为权威版本文件，采用三键双轨格式：
   - `DEBOPTI_SEMVER="X.Y.Z[-prerelease]"`（主版本；历史基线未切换前可缺省）
   - `DEBOPTI_CALVER="YY.MM.DD"`（日历兼容键）
   - `DEBOPTI_BUILD="<lowercase-alnum>"`（短构建标识，与 `scripts/common.sh` 中 `VERSION_ID` 严格一致）
   - **严禁**在磁盘 `VERSION` 中写入 `DEBOPTI_VERSION=`（旧版 VPS 更新器仅认 `YYYY.MM.DD.N`，写入该键会导致校验中止；保留 `DEBOPTI_CALVER` + `DEBOPTI_BUILD` 可保证所有历史更新器平滑升级）。
3. **自动维护禁手改**：**严禁**手工编辑 `VERSION`、`scripts/common.sh` 中的 `VERSION_ID` 或 `CHANGELOG.md` 的发布元数据；一律通过 `tools/release.sh`（与 `tools/github-push.sh`）原子更新并由 `tools/release.sh verify` 门禁校验。

## §3 Conventional Commits 1.0.0 与 Keep a Changelog 1.1.0 规范

### 3.1 提交消息规范（Conventional Commits 1.0.0）

格式：`<type>[(<scope>)][!]: <summary>`，破坏性变更在标题加 `!` 或在正文脚注写 `BREAKING CHANGE: <说明>`。

| Type | 语义 | 自动版本递增（`>=1.0.0`） | 自动版本递增（`0.x`） | Changelog 分组 |
|---|---|---|---|---|
| `feat!` / `BREAKING CHANGE` | 破坏性变更 | `major` | `minor` | `Changed`（前缀 `**BREAKING**: `） |
| `feat` | 新功能 | `minor` | `minor` | `Added`（安全 scope 归入 `Security`） |
| `fix` | 缺陷修复 | `patch` | `patch` | `Fixed`（安全 scope 归入 `Security`） |
| `perf` / `refactor` | 性能优化 / 重构 | `patch`（`perf`） | `patch`（`perf`） | `Changed` |
| `revert` | 回滚变更 | `patch` | `patch` | `Removed` |
| `docs` / `test` / `ci` / `build` / `chore` / `style` | 文档、测试、CI、维护 | `none`（不自动递增） | `none` | 默认省略（`--include-all` 可显式纳入） |

安全类 scope 包括 `security`、`sec`、`fail2ban`、`ssh`、`nftables`，其 `feat`/`fix` 自动归入 `Security` 分组。首次从 CalVer 基线迁移至 SemVer 时，`tools/release.sh` 自动推荐 bootstrap 版本 `1.0.0`。

### 3.2 变更日志规范（Keep a Changelog 1.1.0）

1. `CHANGELOG.md` 顶部固定维护 `# Changelog` 标准头与 `## [Unreleased]` 小节。
2. 每个正式版本以 `## [X.Y.Z] - YYYY-MM-DD` 标题倒序排列，子标题按标准顺序使用 `Added`、`Changed`、`Deprecated`、`Removed`、`Fixed`、`Security`。
3. 执行 `tools/release.sh bump` 时，自动提升 `## [Unreleased]` 中已整理的条目并与提交日志去重合并，完整保留历史 CalVer 条目，并在文件末尾自动维护 `[Unreleased]` 与各版本 GitHub compare/tag 引用链接。

## §4 Agent 自主版本与发布操作流程（SOP）

当完成阶段性功能、修复或 PM 要求发版/推送时，Agent 按以下闭环自主执行：

1. **演练规划（Plan）**：运行 `make release-plan`（或 `bash tools/release.sh plan --json`）只读预览待发布 SemVer、CalVer、Build、目标 tag 与 Changelog 摘要。
2. **质量门禁（Gate）**：运行 `make check`（含 `bash -n`、`py_compile`、`shellcheck`、全部宿主单元测试、双受众格式检查、仓库卫生与 `release verify`），确保零失败。
3. **原子升版（Bump）**：
   - 先用 `bash tools/release.sh bump --dry-run` 预演；确认无误后执行 `bash tools/release.sh bump`（可按需指定 `--bump major|minor|patch`、`--version X.Y.Z` 或 `--prerelease rc.1`）。
   - 或在推送命令直接附带 `--bump auto|minor|patch` / `--version X.Y.Z` 由 `github-push.sh` 一体化执行并折叠提交。
4. **一致性复核（Verify）**：运行 `make release-verify`（必要时附加 `--check-commits` 校验提交规范），确认 `VERSION`、`VERSION_ID` 与 `CHANGELOG.md` 完全一致。
5. **脱敏推送与单一原子发布源（Push & Release）**：
   - `tools/github-push.sh` **仅**负责脱敏净化并推送 `main` 分支与不可变 `vX.Y.Z` annotated tag，同时将已推送 tag 非破坏性地同步回本地 `main`（`_sync_local_tag`），防止本地 tag 漂移；**严禁**在 `github-push.sh` 中调用 `gh release create` 或上传残缺归档。
   - `.github/workflows/release.yml` 是**唯一** GitHub Release 原子发布者：收到 `vX.Y.Z*` tag 后复跑严格门禁、`release.sh verify` 与全历史秘密扫描，并发构建 `amd64`/`arm64` 静态 musl 二进制与洁净生产资产包 `debopty-vX.Y.Z.tar.gz`，汇总生成统一 `SHA256SUMS`，并在 `publish` 作业中带显式 `--repo` / `GH_REPO` 上下文幂等创建或更新 Release（预发布自动标记 `--prerelease`）。
6. **不可变与热修复规则**：
   - **Tag 不可变**：严禁删除、覆盖或改写已发布的远端 `vX.Y.Z` tag；发现问题一律递增 `patch`（如 `vX.Y.(Z+1)`）重新发版。
   - **预发布通道**：高风险改动先发 `-rc.1` / `-rc.2` 预发布验证，稳定后再切正式版 `X.Y.Z`。

## §5 GitHub 脱敏推送协议（阶段 A–E）

当 PM 要求「脱敏推送」「推送到 GitHub」「执行 github-push」时，Agent **必须**按以下五阶段自动执行，**严禁**直接运行 `jj git push` 或 `git push origin`。

### 阶段 A — 推送前准备
1. 确认代码、`docs/`、`templates/` 改动已全部落盘，运行 `make check`（含工作树秘密字面量扫描）全部通过；推送前脚本亦会强制复核 `tools/release.sh verify` 与 `tools/verify_repo_hygiene.py`。
2. 按 Conventional Commits 1.0.0（`<type>(<scope>): <summary>`，严禁 `scope` 内含逗号）对含代码的 change 执行 `jj describe -m ...`；若 `jj diff --stat` 非空，先折叠提交后再 `describe`。
3. 判定目标 `PUSH_REV`：若 `@` 为 empty change，则 `PUSH_REV='@-'`；否则 `PUSH_REV='@'`。
4. 同步 Git `main` 书签（沙盒克隆检出 Git `main` 而非 `jj @`）：
   ```bash
   jj bookmark set main -r "${PUSH_REV}" --allow-backwards
   jj git export
   git checkout main
   ```
   确认 `git rev-parse main` 与 `PUSH_REV` 一致；若尚未升版，通过 `tools/release.sh bump` 或向 `github-push.sh` 传入 `--bump` 处理，严禁手工改写 `VERSION`/`VERSION_ID`/`CHANGELOG.md`。

### 阶段 B — 执行脱敏推送
- **历史净化与推送前阻断门禁**：沙盒强制通过 `git-filter-repo` 按规则重写已知历史测试秘密字面量（缺少 `git-filter-repo` 时 fail-closed 中止）；推送网络前在沙盒执行 `python3 -B tools/verify_repo_hygiene.py --scan-git-history HEAD` 扫描全量 Git 历史，任意命中即刻阻断。推送 `vX.Y.Z` tag 成功后自动同步本地 tag（`_sync_local_tag`）。
- 预演（不连 SSH、不写远端）：`bash tools/github-push.sh --dry-run`
- 常规推送（保留已就绪 SemVer 身份）：`bash tools/github-push.sh`
- 自动升版并推送：`bash tools/github-push.sh --bump auto`（或 `--version X.Y.Z`、`--prerelease rc.1`）
- 强制重写远端历史：`bash tools/github-push.sh --force`
- 仅补发 tag：`bash tools/github-push.sh --tag-only`

### 阶段 C — 认证与传输优先级（GitHub 插件优先，SSH 回退）
1. **GitHub 插件优先（推荐，免 SSH 手动解锁）**：当系统已安装并登录 `gh` / `gh-helper` 且未通过 `DEBOPTY_GITHUB_REMOTE` 显式强制 SSH 地址时，脚本自动匹配仓库所属账号（`gh-helper account switch` / `gh auth switch`），通过 `gh-helper setup --setup-git` / `gh auth setup-git` 启用 HTTPS 凭据助手（`https://github.com/<owner>/<repo>.git`），并直接调用 `gh api` 与 `gh run list` 完成远端版本与 Actions 复核（零 `curl`、零 `ssh-agent` 轮子）。
2. **SSH 回退机制**：仅当 `gh` 不可用/未登录或显式传入 SSH 远端地址时，按以下 5 级顺序自动加载私钥（全程仅需解密一次）：
   - 密钥已在 `ssh-agent` 中：自动跳过。
   - 已配置 systemd 用户加密凭证：自动从 `~/.config/credstore/debopti-github-ssh.cred` 解密（一次性初始化命令：`bash tools/github-ssh-cred-setup.sh` 或 `--passphrase-stdin`；严禁提交 `.cred` 文件）。
   - 本地存在 `/dev/tty`：终端交互提示输入。
   - 无 TTY 且无凭证的 AI 对话环境：脚本输出 `DEBOPTI_SSH_PASSPHRASE_REQUIRED`，由 PM 在对话中提供密码后以 `SSH_PASSPHRASE='<密码>' bash tools/github-push.sh` 注入（严禁写入仓库或提交历史）。
   - 标准输入管道：`printf '%s\n' '<密码>' | bash tools/github-push.sh --passphrase-stdin`。

若既未登录 `gh` 又未找到私钥，引导 PM 执行 `gh auth login` 或在 `~/.ssh/config` 中为 `github.com` 配置 `IdentityFile`。

### 阶段 D — 推送后复核
1. 确认脚本输出「推送成功完成」且退出码为 `0`，并已将 `vX.Y.Z` annotated tag 同步至本地 `main`。
2. 复核脚本打印的 `SemVer` / `CalVer` / `Build` 与 `v*` tag，以及 GitHub Actions `.github/workflows/release.yml` 是否成功发布 `debopty-vX.Y.Z.tar.gz`、双架构 musl 二进制与统一 `SHA256SUMS`。
3. 确认仓库与远端历史中不包含任何人类用户个人身份信息（PII）、私钥、明文秘密字面量或凭证文件。

### 阶段 E — 收尾与中文汇报
若 `@` 非空则执行 `jj new` 开启新工作 change，并用简体中文向 PM 汇报：是否跳过、是否成功、发布的 SemVer / CalVer / Build 与 tag、是否使用 `--force`、当前 `@` 状态及任何异常修复建议。

## §6 公开范围、隐私保护与禁止项

1. **公开 Vibe-Coding 协作资产**：已取消隐藏 Vibe-Coding 痕迹的限制；`.agents/`、`tools/`、`tests/`、根目录 `AGENTS.md`、兼容入口、`Makefile` 与 `.github/` 均随主分支完整公开推送至 GitHub，以便 GitHub Actions CI 自动执行全部单元测试。
2. **人类隐私与零秘密字面量底线**：
   - 推送前门禁严格拦截人类用户个人身份信息（PII）、SSH 私钥、API Token、`.cred` / `.pem` / `.key` 等敏感凭证，确保零泄露。
   - **测试夹具零明文秘密红线**：严禁在源码、单元测试、模板或文档中硬编码触发 Secret Scanning 的静态字面量（如 PEM 私钥头、API Token、JWT 片段或含凭据 URL）；所有脱敏/拦截测试必须在运行期动态拼接构造，由 `tools/verify_repo_hygiene.py`（工作树默认扫描及 `--scan-git-history HEAD` 全历史扫描）强制把关（多代理并发验收要求见 `80 §3`）。
3. **可选配置**：默认仓库 `REMOTE_REPO` 支持 `DEBOPTY_REPO` 覆盖，默认远程 `GITHUB_REMOTE` 支持 `DEBOPTY_GITHUB_REMOTE` 覆盖；`ENABLE_GPG_SIGNING` 默认 `false`（启用前须确保 `gpg-agent` 已缓存密码）。
4. **严格禁止**：禁止提交或推送任何人类用户身份信息（PII）与密钥凭证；禁止覆盖已发布的远端 tag。
