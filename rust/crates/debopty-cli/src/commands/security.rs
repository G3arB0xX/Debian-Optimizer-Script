use std::env;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use clap::Subcommand;
use debopty_core::CoreError;
use debopty_core::security::SecurityContext;
use debopty_core::security::fail2ban::{setup_fail2ban, setup_security};
use debopty_core::security::firewall::{
    add_fw_rule, patch_nftables_ndp, remove_fw_rule, update_ssh_firewall,
};
use debopty_core::security::ssh::{
    NonInteractivePrompter, SshPrompter, StdioPrompter, check_ssh_security,
};
use debopty_core::sys::RealCommandRunner;
use debopty_tui::is_non_interactive;

use crate::{CliError, env_truthy, non_empty_env};

#[derive(Debug, Subcommand)]
pub enum SecurityAction {
    /// 添加模块化 nftables 防火墙规则并同步 Fail2ban
    FwAdd {
        /// 端口、端口列表或端口范围 (例如 443, 80,443, 11010-11015)
        port: String,
        /// 协议: tcp | udp | tcp/udp
        #[arg(long, default_value = "tcp", value_name = "PROTO")]
        proto: String,
        /// 规则注释/文件名标识
        #[arg(long, default_value = "", value_name = "NAME")]
        comment: String,
        /// 根目录前缀 (用于无特权隔离测试)
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 nft/systemctl 等特权系统命令
        #[arg(long)]
        no_sys: bool,
    },
    /// 按注释名称移除模块化 nftables 规则并同步 Fail2ban
    FwRemove {
        /// 规则注释/文件名标识
        comment: String,
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 nft/systemctl 等特权系统命令
        #[arg(long)]
        no_sys: bool,
    },
    /// 更新主 nftables.conf 中的 SSH 管理端口并同步 Fail2ban
    FwUpdateSsh {
        /// 新的 SSH 监听端口
        port: u16,
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 nft/systemctl 等特权系统命令
        #[arg(long)]
        no_sys: bool,
    },
    /// 幂等修补 nftables.conf 中的 IPv6 NDP 放行规则
    NdpPatch {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 systemctl reload nftables
        #[arg(long)]
        no_sys: bool,
    },
    /// 部署 Fail2ban 最佳实践配置、公网端口同步服务与定时器
    F2bSetup {
        /// 指定 SSH 端口 (省略时自动探测)
        #[arg(long, value_name = "PORT")]
        ssh_port: Option<u16>,
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 apt/systemctl 系统调用
        #[arg(long)]
        no_sys: bool,
    },
    /// 执行 SSH 零信任安全加固 (普通用户/Ed25519/高位端口/锁定 root)
    SshHarden {
        /// 显式启用无人值守非交互模式 (需配合 --pubkey，禁止静默锁定)
        #[arg(long)]
        non_interactive: bool,
        /// 指定写入目标用户 authorized_keys 的 Ed25519 公钥
        #[arg(long, value_name = "ED25519_KEY")]
        pubkey: Option<String>,
        /// 指定创建或复用的普通管理员用户名
        #[arg(long, value_name = "USER")]
        username: Option<String>,
        /// 指定新建用户的密码 (也可通过 DEBOPTY_SSH_PASSWORD 传入)
        #[arg(long, value_name = "PASS")]
        password: Option<String>,
        /// 仅限自动化测试: 允许非交互模式自动生成临时密钥与测试密码
        #[arg(long, hide = true)]
        allow_test_ephemeral_auth: bool,
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 systemctl/useradd/passwd 等特权命令
        #[arg(long)]
        no_sys: bool,
    },
    /// 初始化完整安全引擎 (清理 UFW + nftables + Fail2ban)
    Setup {
        /// 根目录前缀
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
        /// 跳过 apt/systemctl/nft 特权命令
        #[arg(long)]
        no_sys: bool,
    },
}

fn resolve_ssh_harden_non_interactive(
    flag_ni: bool,
    env_ni: bool,
    stdin_is_tty: bool,
) -> Result<bool, CliError> {
    let explicit_ni = flag_ni || env_ni;
    if !explicit_ni && !stdin_is_tty {
        return Err(CliError::Core(CoreError::ValidationFailed(
            "非交互终端禁止静默执行 SSH 加固；请使用交互式 TTY，或显式传入 --non-interactive --pubkey <ED25519_KEY>".to_string(),
        )));
    }
    Ok(explicit_ni)
}

fn build_security_context(
    root: Option<&Path>,
    no_sys: bool,
    explicit_config_path: Option<&Path>,
    template_dir: Option<&Path>,
) -> SecurityContext {
    let mut ctx = if let Some(r) = root {
        SecurityContext::with_root(r)
    } else if let Some(env_root) = non_empty_env("DEBOPTY_F2B_ROOT") {
        SecurityContext::with_root(env_root)
    } else {
        SecurityContext::default()
    };

    ctx.no_sys = no_sys || env_truthy("DEBOPTY_F2B_NO_SYS");
    ctx.non_interactive = is_non_interactive() || env_truthy("NON_INTERACTIVE");
    ctx.f2b_backend_override = non_empty_env("DEBOPTY_F2B_BACKEND");
    ctx.template_dir = template_dir
        .map(Path::to_path_buf)
        .or_else(|| non_empty_env("DEBOPTY_F2B_TEMPLATE_DIR").map(PathBuf::from));
    if let Some(cfg) = explicit_config_path {
        ctx.config_path = cfg.to_path_buf();
    }
    ctx.binary_path = env::current_exe().ok();
    ctx
}

pub fn handle_security(
    action: &SecurityAction,
    explicit_config_path: Option<&Path>,
    template_dir: Option<&Path>,
) -> Result<i32, CliError> {
    let runner = RealCommandRunner;

    match action {
        SecurityAction::FwAdd {
            port,
            proto,
            comment,
            root,
            no_sys,
        } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let rule_file = add_fw_rule(&ctx, port, proto, comment, &runner)?;
            println!("防火墙规则已写入: {}", rule_file.display());
            Ok(0)
        }
        SecurityAction::FwRemove {
            comment,
            root,
            no_sys,
        } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let removed = remove_fw_rule(&ctx, comment, &runner)?;
            if removed {
                println!("已移除防火墙规则: {comment}");
            } else {
                println!("未找到对应防火墙规则: {comment}");
            }
            Ok(0)
        }
        SecurityAction::FwUpdateSsh { port, root, no_sys } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            update_ssh_firewall(&ctx, *port, &runner)?;
            println!("SSH 防火墙规则已更新至端口: {port}");
            Ok(0)
        }
        SecurityAction::NdpPatch { root, no_sys } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let patched = patch_nftables_ndp(&ctx, &runner)?;
            if patched {
                println!("IPv6 NDP 防火墙规则已修补。");
            } else {
                println!("IPv6 NDP 防火墙规则已就绪或无需修补。");
            }
            Ok(0)
        }
        SecurityAction::F2bSetup {
            ssh_port,
            root,
            no_sys,
        } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let report = setup_fail2ban(&ctx, *ssh_port, &runner)?;
            println!(
                "Fail2ban 部署完成 (SSH={}, 公网端口={})",
                report.ssh_port, report.all_public_ports_f2b
            );
            Ok(0)
        }
        SecurityAction::SshHarden {
            non_interactive,
            pubkey,
            username,
            password,
            allow_test_ephemeral_auth,
            root,
            no_sys,
        } => {
            let explicit_ni = resolve_ssh_harden_non_interactive(
                *non_interactive,
                env_truthy("NON_INTERACTIVE"),
                io::stdin().is_terminal(),
            )?;
            let mut ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let non_interactive_prompter = NonInteractivePrompter {
                username: username.clone(),
                password: password
                    .clone()
                    .or_else(|| non_empty_env("DEBOPTY_SSH_PASSWORD")),
                ssh_pubkey: pubkey.clone(),
            };
            let stdio_prompter = StdioPrompter;
            let prompter: &dyn SshPrompter = if explicit_ni {
                ctx.non_interactive = true;
                ctx.allow_test_ephemeral_auth = *allow_test_ephemeral_auth;
                &non_interactive_prompter
            } else {
                ctx.non_interactive = false;
                ctx.allow_test_ephemeral_auth = false;
                &stdio_prompter
            };
            let report = check_ssh_security(&ctx, prompter, &runner)?;
            if report.skipped {
                println!("已跳过 SSH 安全加固。");
            } else if report.rolled_back {
                println!("SSH 连通性验证未通过，已回滚至端口 {}。", report.old_port);
                return Ok(1);
            } else {
                println!(
                    "SSH 加固完成 (用户={}, 端口={} -> {}, locked_down={})",
                    report.user.as_deref().unwrap_or("-"),
                    report.old_port,
                    report.new_port,
                    report.locked_down
                );
            }
            Ok(0)
        }
        SecurityAction::Setup { root, no_sys } => {
            let ctx = build_security_context(
                root.as_deref(),
                *no_sys,
                explicit_config_path,
                template_dir,
            );
            let report = setup_security(&ctx, &runner)?;
            println!(
                "安全引擎初始化完成 (SSH={}, 公网端口={})",
                report.ssh_port, report.all_public_ports_f2b
            );
            Ok(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_harden_non_tty_without_explicit_flag_fails_closed() {
        let err = resolve_ssh_harden_non_interactive(false, false, false)
            .expect_err("non-TTY without explicit flag must fail closed");
        assert!(matches!(
            err,
            CliError::Core(CoreError::ValidationFailed(_))
        ));
        assert!(err.to_string().contains("--non-interactive"));

        assert!(!resolve_ssh_harden_non_interactive(false, false, true).expect("interactive TTY"));
        assert!(resolve_ssh_harden_non_interactive(true, false, false).expect("explicit flag"));
        assert!(resolve_ssh_harden_non_interactive(false, true, false).expect("explicit env"));
        assert!(resolve_ssh_harden_non_interactive(true, false, true).expect("flag on TTY"));
    }
}
