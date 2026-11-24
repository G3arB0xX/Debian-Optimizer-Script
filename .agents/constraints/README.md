# 约束索引

[仓库总入口](../../AGENTS.md) | [约束总索引](../CONSTRAINTS.md) | [INDEX.md](INDEX.md) | [手动配置文档](../../docs/README.md)

本目录存放 `debopty` 项目的全部工程与治理约束。规则按领域拆分，每条规则只在一处详写，其他位置以 `<层号> §<节号>` 交叉引用，不重复抄录。所有会话、子代理与自动化流程须先读 [00_general.md](00_general.md) 与 [50_context_and_docs.md](50_context_and_docs.md)，再按任务类型通读所需分层文件。各层同等有效，工作范围扩展时回到此处补读对应层。

## 分层文件

| 文件 | 内容 | 必读范围 |
|---|---|---|
| [00_general.md](00_general.md) | 角色与 PM 画像、全包揽与零占位符、计划模式、代码连续性、中文精简文风、项目身份与双受众文档契约 | 全员必读 |
| [10_bash_and_templates.md](10_bash_and_templates.md) | 严格 Bash 与防御性编程、`ensure_runtime_home`、`templates/` 与 `render_template`、幂等与原子还原、无 `curl`/`wget` 自举、终端工具链快捷键规范、核心架构与组件不变量 | 凡改脚本、模板、TUI 或组件必读 |
| [20_security_and_systemd.md](20_security_and_systemd.md) | SSH 加固与回滚、nftables 模块化防火墙、Fail2ban 最佳实践与全公网端口自动同步、Systemd 沙盒与最小权限、Rootless Podman 与证书组 | 凡改安全、防火墙、服务单元或 Podman 必读 |
| [30_release_and_vcs.md](30_release_and_vcs.md) | `jj` + `git` 开发流、生产纯净性、SemVer 2.0.0 + CalVer/Build 双轨身份、Conventional Commits & Keep a Changelog、`tools/release.sh` Agent 自主发布 SOP 与 `tools/github-push.sh` 阶段 A–E 脱敏推送协议 | 凡执行提交、版本、Changelog 与发布推送必读 |
| [40_testing_and_ci.md](40_testing_and_ci.md) | `make check` / `make release-verify` / `tools/check.sh` 与 GitHub Actions CI/CD（`check.yml`、`release.yml`）、宿主无特权单元测试（含 `release-test.sh`）、Podman 容器测试与嵌套容器限制 | 凡编写测试、改动模块或维护 CI/发布门禁必读 |
| [50_context_and_docs.md](50_context_and_docs.md) | `L0 -> L1 -> L2` 渐进式披露、目录分片阈值、`docs/` 手动教程增量同步、日期禁令与平台中立性 | 全员必读 |
| [60_rust_and_reconcile.md](60_rust_and_reconcile.md) | Pure Rust 四 crate 与共存归档、TUI 双后端与终端恢复、烂摊子稳态接管、用户自定义保留、原子权限与 `Fail-Closed`、统一错误码与流分离日志 | 凡改 Rust、TUI、接管引擎或诊断日志必读 |
| [70_agent_workspace_and_credentials.md](70_agent_workspace_and_credentials.md) | Agent-First CLI 契约与退出码 `21..26`、v2 VPS Agent 工作环境与 CoALA 弹性记忆（Write Gate/BM25/追踪策略）、Harness 通用化（14 适配器与覆盖槽位）、旧记忆与旧约束无损迁移回滚、凭据安全存储与双受众约束 | 凡改 Agent CLI、VPS 工作环境模板、记忆/迁移或凭据治理必读 |
| [80_swarm_orchestration.md](80_swarm_orchestration.md) | 多代理并发协作红线：契约先行与无交集分片、只读即时审计与路径限定原子提交、Cargo 并发隔离、运行期测试凭据拼接与全历史卫生门禁、多轮红蓝对抗与混沌仿真、仓库外部 `/tmp` 隔离 | 凡执行多代理并行开发、红蓝对抗审查或集成合入必读 |

## 按工作类型的必读

| 工作类型 | 必读层 | 视情况加读 |
|---|---|---|
| 新增或修改核心脚本、TUI、模板、系统调优与组件模块 | 00、10、40、50 | 涉及端口、服务单元或权限加读 20；涉及 Rust/TUI 加读 60 |
| 开发或维护 Rust 引擎、稳态接管（`reconcile`）与诊断日志 | 00、10、20、40、50、60 | 涉及 Agent CLI 或凭据存储加读 70 |
| 维护 VPS Agent 预设工作环境、模板、检查点或凭据模块 | 00、20、50、60、70 | 涉及测试或发布过滤加读 30、40 |
| 修改安全加固、SSH、nftables、Fail2ban 或 Podman | 00、10、20、40、50 | 涉及凭据或加密注入加读 70 |
| 撰写或同步 `docs/` 手动教程与 `README*.md` | 00、50 | 对应模块涉及的技术层（10 或 20） |
| 编写测试脚本或调整 `Makefile`、`tools/`、CI 工作流 | 00、40、50 | 涉及发布过滤清单加读 30 |
| 执行 `jj` 提交、同步 `main` 与 GitHub 脱敏推送 | 00、30、40 | — |
| 多代理并行开发、红蓝对抗审查与集成合入 | 00、30、40、50、80 | 涉及 Rust 构建加读 60；涉及凭据或工作环境加读 70 |
| 修订治理规范与约束文件 | 00、50 | 受影响的对应层 |

## 修改约束

新约束写入所属层的对应章节，严禁在约束文件内写修改日期。若新增分层或工作类型，同步更新本文件、[INDEX.md](INDEX.md) 与 [../CONSTRAINTS.md](../CONSTRAINTS.md)。
