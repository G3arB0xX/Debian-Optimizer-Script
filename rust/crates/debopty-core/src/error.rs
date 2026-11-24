use thiserror::Error;

pub const EXIT_WRITE_GATE_REJECT: u8 = 21;
pub const EXIT_MEMORY_BLOAT: u8 = 22;
pub const EXIT_MEMORY_SECRET_LEAK: u8 = 23;
pub const EXIT_AUDIT_NONSTANDARD: u8 = 24;
pub const EXIT_MIGRATION_CONFLICT: u8 = 25;
pub const EXIT_SECRET_DETECTED: u8 = 26;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid version: {0}")]
    InvalidVersion(String),

    #[error("invalid version bump: {0}")]
    InvalidBump(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("template not found: {0}")]
    TemplateNotFound(String),

    #[error("command '{prog}' failed: {message}")]
    CommandFailed { prog: String, message: String },

    #[error("command '{prog}' timed out after {secs}s")]
    CommandTimeout { prog: String, secs: u64 },

    #[error("validation failed: {0}")]
    ValidationFailed(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("HTTP {status} for {url}")]
    HttpStatus { url: String, status: u16 },

    #[error("system error: {0}")]
    System(String),

    #[error("security error: {0}")]
    Security(String),

    #[error("reconcile error: {0}")]
    Reconcile(String),

    #[error("app '{app}' error: {message}")]
    App { app: String, message: String },

    #[error("agent policy [{kind}] (exit {code}): {message}")]
    AgentPolicy {
        code: u8,
        kind: &'static str,
        message: String,
    },
}

impl CoreError {
    pub fn agent_policy(code: u8, kind: &'static str, message: impl Into<String>) -> Self {
        Self::AgentPolicy {
            code,
            kind,
            message: message.into(),
        }
    }

    /// Deterministic CLI exit code for agent workspace policy errors (`21..=26`).
    pub fn agent_exit_code(&self) -> Option<u8> {
        match self {
            Self::AgentPolicy { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// Stable machine-readable error taxonomy code (`E1xxx..E5xxx`).
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Io(_) => "E1001",
            Self::ValidationFailed(_) => "E1003",
            Self::InvalidVersion(_) => "E2001",
            Self::InvalidBump(_) => "E2002",
            Self::InvalidConfig(_) => "E2003",
            Self::TemplateNotFound(_) => "E2004",
            Self::Network(_) => "E3001",
            Self::HttpStatus { .. } => "E3002",
            Self::System(_) => "E4001",
            Self::CommandFailed { .. } => "E4002",
            Self::CommandTimeout { .. } => "E4003",
            Self::App { .. } => "E4004",
            Self::Security(_) => "E5001",
            Self::Reconcile(_) => "E5002",
            Self::AgentPolicy { code, .. } => match *code {
                EXIT_WRITE_GATE_REJECT => "E5021",
                EXIT_MEMORY_BLOAT => "E5022",
                EXIT_MEMORY_SECRET_LEAK => "E5023",
                EXIT_AUDIT_NONSTANDARD => "E5024",
                EXIT_MIGRATION_CONFLICT => "E5025",
                EXIT_SECRET_DETECTED => "E5026",
                _ => "E5020",
            },
        }
    }

    /// Actionable Simplified Chinese remediation guidance for operators.
    pub fn remediation_hint(&self) -> &'static str {
        match self {
            Self::Io(_) => {
                "请检查目标文件路径、磁盘空间与当前用户读写权限（系统目录通常需要 root/sudo）。"
            }
            Self::ValidationFailed(_) => {
                "请核对输入参数格式与取值范围，使用 --help 查看合法选项说明。"
            }
            Self::InvalidVersion(_) => {
                "请检查 VERSION 文件格式，确保包含合法的 SemVer 2.0.0 或 CalVer 版本号。"
            }
            Self::InvalidBump(_) => {
                "请使用合法的版本递增类型（major、minor、patch 或显式 X.Y.Z）。"
            }
            Self::InvalidConfig(_) => {
                "请检查 /etc/debopti/debopti.conf 键值语法，可运行 `debopty reconcile --domain legacy --apply` 自动修复。"
            }
            Self::TemplateNotFound(_) => {
                "请确认模板相对路径正确，或运行 `debopty template list` 查看可用模板。"
            }
            Self::Network(_) => {
                "请检查网络连通性、DNS 解析及代理配置，必要时运行 `debopty network dns-check` 校准。"
            }
            Self::HttpStatus { .. } => {
                "远端镜像返回异常状态码，请稍后重试或切换中国大陆/海外镜像模式。"
            }
            Self::System(_) => "请确认当前为受支持的 Debian 系统并检查相关系统服务状态与日志。",
            Self::CommandFailed { .. } => {
                "请确认所需外部命令已安装且具备执行权限，可附加 `-vv` 查看详细 stderr 诊断。"
            }
            Self::CommandTimeout { .. } => {
                "外部命令执行超时，请检查网络、锁文件或宿主机负载后重试。"
            }
            Self::App { .. } => {
                "应用部署或管理操作失败，请检查网络连接、软件包源及宿主机权限，并附加 -vv 查看详细 stderr 诊断。"
            }
            Self::Security(_) => {
                "安全操作已中止以防断连或规则损坏，请检查 sshd -t / nft -c -f 输出后再试。"
            }
            Self::Reconcile(_) => {
                "系统稳态收敛已保护性回滚，请排查告警项或使用 `debopty reconcile --restore <备份目录>` 恢复。"
            }
            Self::AgentPolicy { code, .. } => match *code {
                EXIT_WRITE_GATE_REJECT => {
                    "记忆写入被 Write Gate 拒绝：请检查 10 项必填 CoALA Frontmatter 字段、来源溯源或是否与现有主题冲突。"
                }
                EXIT_MEMORY_BLOAT => {
                    "记忆内容超出防膨胀上限（TOC <=150行/12KB，分片 <=200行/16KB，代码块 <=80行），请拆分后重试。"
                }
                EXIT_MEMORY_SECRET_LEAK => {
                    "检测到明文密钥或凭据，已阻断记忆写入；请将敏感值存入 `.agents/credentials/` 并仅引用逻辑名称。"
                }
                EXIT_AUDIT_NONSTANDARD => {
                    "工作区审计发现旧版文件、垫片漂移或厂商耦合，请运行 `debopty agent migrate` 执行无损迁移。"
                }
                EXIT_MIGRATION_CONFLICT => {
                    "检测到手工修改的受管垫片或迁移冲突，请附加 `--harvest` 无损收割至 `90_project_legacy_ssot.md` 后再切换。"
                }
                EXIT_SECRET_DETECTED => {
                    "审计/迁移阶段在凭据目录外发现明文密钥或敏感文件，已安全阻断；请先清理或迁移至 `.agents/credentials/`。"
                }
                _ => "请检查工作区规范与审计日志 `.agents/logs/audit.jsonl`。",
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn every_core_error_variant_has_stable_code_and_non_empty_hint() {
        let samples: Vec<(CoreError, &str)> = vec![
            (CoreError::Io(io::Error::other("disk full")), "E1001"),
            (CoreError::ValidationFailed("bad arg".into()), "E1003"),
            (CoreError::InvalidVersion("x".into()), "E2001"),
            (CoreError::InvalidBump("x".into()), "E2002"),
            (CoreError::InvalidConfig("x".into()), "E2003"),
            (CoreError::TemplateNotFound("x".into()), "E2004"),
            (CoreError::Network("x".into()), "E3001"),
            (
                CoreError::HttpStatus {
                    url: "https://example.com".into(),
                    status: 503,
                },
                "E3002",
            ),
            (CoreError::System("x".into()), "E4001"),
            (
                CoreError::CommandFailed {
                    prog: "nft".into(),
                    message: "fail".into(),
                },
                "E4002",
            ),
            (
                CoreError::CommandTimeout {
                    prog: "apt-get".into(),
                    secs: 30,
                },
                "E4003",
            ),
            (
                CoreError::App {
                    app: "realm".into(),
                    message: "install failed".into(),
                },
                "E4004",
            ),
            (CoreError::Security("x".into()), "E5001"),
            (CoreError::Reconcile("x".into()), "E5002"),
            (
                CoreError::agent_policy(EXIT_WRITE_GATE_REJECT, "WRITE_GATE_REJECT", "bad schema"),
                "E5021",
            ),
            (
                CoreError::agent_policy(EXIT_MEMORY_BLOAT, "MEMORY_BLOAT", "too big"),
                "E5022",
            ),
            (
                CoreError::agent_policy(EXIT_MEMORY_SECRET_LEAK, "MEMORY_SECRET_LEAK", "leak"),
                "E5023",
            ),
            (
                CoreError::agent_policy(EXIT_AUDIT_NONSTANDARD, "AUDIT_NONSTANDARD", "drift"),
                "E5024",
            ),
            (
                CoreError::agent_policy(EXIT_MIGRATION_CONFLICT, "MIGRATION_CONFLICT", "dirty"),
                "E5025",
            ),
            (
                CoreError::agent_policy(EXIT_SECRET_DETECTED, "SECRET_DETECTED", "secret"),
                "E5026",
            ),
        ];

        for (err, expected_code) in samples {
            assert_eq!(err.error_code(), expected_code);
            assert!(
                !err.remediation_hint().trim().is_empty(),
                "empty hint for {expected_code}"
            );
        }
        assert_eq!(
            CoreError::agent_policy(EXIT_SECRET_DETECTED, "SECRET_DETECTED", "x").agent_exit_code(),
            Some(26)
        );
        assert_eq!(CoreError::System("x".into()).agent_exit_code(), None);
    }
}
