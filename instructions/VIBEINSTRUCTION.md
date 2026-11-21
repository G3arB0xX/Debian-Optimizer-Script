# VIBEINSTRUCTION（已迁移）

本文档原有的单体规范已按分层治理架构无损拆分，不再作为权威来源维护。

## 请改读以下入口

- **Agent 统一总入口**：[../AGENTS.md](../AGENTS.md)
- **Agent 约束总索引**：[CONSTRAINTS.md](CONSTRAINTS.md)
- **人类可读分层约束索引**：[constraints/README.md](constraints/README.md)
  - [00_general.md](constraints/00_general.md)：总则、角色、工作流与双受众契约
  - [10_bash_and_templates.md](constraints/10_bash_and_templates.md)：Bash 规范、模板引擎、自举安装与终端工具链
  - [20_security_and_systemd.md](constraints/20_security_and_systemd.md)：SSH、nftables、Fail2ban、Systemd 与 Podman 安全约束
  - [30_release_and_vcs.md](constraints/30_release_and_vcs.md)：`jj`/`git` 工作流、版本标识与 GitHub 脱敏推送协议
  - [40_testing_and_ci.md](constraints/40_testing_and_ci.md)：本地门禁、CI、宿主单元测试与 Podman 容器测试
  - [50_context_and_docs.md](constraints/50_context_and_docs.md)：渐进式披露、文档同步、日期禁令与平台中立性
- **历史决策台账**：[decisions.md](decisions.md)
