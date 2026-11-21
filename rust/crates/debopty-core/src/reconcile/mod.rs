pub mod backup;
pub mod firewall;
pub mod ssh;
pub mod system_net;

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::config::DEFAULT_CONFIG_PATH;
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::reconcile::backup::{BackupSession, prefixed};
use crate::sys::CommandRunner;
use crate::template::TemplateEngine;

pub use backup::{BackupEntry, BackupManifest, restore_backup};

pub const MANAGED_FILE_MARKER: &str = "# managed-by-debopty";
pub const LEGACY_MANAGED_MARKER: &str = "# managed-by-debopti";
pub const MANAGED_BLOCK_START_PREFIX: &str = "# >>> debopty-managed:";
pub const MANAGED_BLOCK_END_PREFIX: &str = "# <<< debopty-managed:";

pub const DEFAULT_RECONCILE_BACKUP_DIR: &str = "/var/backups/debopty/reconcile";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReconcileDomain {
    Ssh,
    Firewall,
    Dns,
    Sysctl,
    Limits,
    Swap,
    Logrotate,
    Apt,
    Legacy,
}

impl ReconcileDomain {
    pub const ALL: &'static [ReconcileDomain] = &[
        Self::Ssh,
        Self::Firewall,
        Self::Dns,
        Self::Sysctl,
        Self::Limits,
        Self::Swap,
        Self::Logrotate,
        Self::Apt,
        Self::Legacy,
    ];
}

impl fmt::Display for ReconcileDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ssh => f.write_str("ssh"),
            Self::Firewall => f.write_str("firewall"),
            Self::Dns => f.write_str("dns"),
            Self::Sysctl => f.write_str("sysctl"),
            Self::Limits => f.write_str("limits"),
            Self::Swap => f.write_str("swap"),
            Self::Logrotate => f.write_str("logrotate"),
            Self::Apt => f.write_str("apt"),
            Self::Legacy => f.write_str("legacy"),
        }
    }
}

impl FromStr for ReconcileDomain {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ssh" => Ok(Self::Ssh),
            "firewall" | "fw" | "nft" | "nftables" | "fail2ban" => Ok(Self::Firewall),
            "dns" => Ok(Self::Dns),
            "sysctl" => Ok(Self::Sysctl),
            "limits" => Ok(Self::Limits),
            "swap" => Ok(Self::Swap),
            "logrotate" => Ok(Self::Logrotate),
            "apt" => Ok(Self::Apt),
            "legacy" => Ok(Self::Legacy),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid reconcile domain '{other}': expected one of all, ssh, firewall, dns, sysctl, limits, swap, logrotate, apt, legacy"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReconcileMode {
    Audit,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingStatus {
    Clean,
    PreservedUserCustom,
    WouldFix,
    Fixed,
    Quarantined,
    SkippedNoSys,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileFinding {
    pub domain: ReconcileDomain,
    pub code: String,
    pub severity: FindingSeverity,
    pub status: FindingStatus,
    pub target: String,
    pub summary: String,
    pub backup_rel_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReconcileContext {
    pub root_prefix: PathBuf,
    pub config_path: PathBuf,
    pub backup_base_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub no_sys: bool,
    pub is_cn_region: bool,
    pub domains: Vec<ReconcileDomain>,
    pub f2b_backend_override: Option<String>,
}

impl Default for ReconcileContext {
    fn default() -> Self {
        Self::with_root("/")
    }
}

impl ReconcileContext {
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root_prefix = root.into();
        let config_path = prefixed(&root_prefix, DEFAULT_CONFIG_PATH);
        let backup_base_dir = prefixed(&root_prefix, DEFAULT_RECONCILE_BACKUP_DIR);
        Self {
            root_prefix,
            config_path,
            backup_base_dir,
            template_dir: None,
            no_sys: false,
            is_cn_region: false,
            domains: Vec::new(),
            f2b_backend_override: None,
        }
    }

    pub fn path(&self, abs: &str) -> PathBuf {
        prefixed(&self.root_prefix, abs)
    }

    pub fn effective_domains(&self) -> Vec<ReconcileDomain> {
        if self.domains.is_empty() {
            ReconcileDomain::ALL.to_vec()
        } else {
            self.domains.clone()
        }
    }

    pub fn template_engine(&self) -> TemplateEngine {
        TemplateEngine::new(self.template_dir.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub mode: ReconcileMode,
    pub backup_dir: Option<PathBuf>,
    pub findings: Vec<ReconcileFinding>,
    pub drift_count: usize,
    pub changes_made: usize,
    pub preserved_count: usize,
}

/// Returns `true` if `content` carries either the current or legacy debopty managed-file marker.
pub fn has_managed_marker(content: &str) -> bool {
    content.contains(MANAGED_FILE_MARKER) || content.contains(LEGACY_MANAGED_MARKER)
}

/// Insert or replace a named managed block (`# >>> debopty-managed:<id>` .. `# <<< debopty-managed:<id>`)
/// inside `existing` while preserving all surrounding user content verbatim.
pub fn upsert_managed_block(existing: &str, block_id: &str, body: &str) -> String {
    let start_marker = format!("{MANAGED_BLOCK_START_PREFIX}{block_id}");
    let end_marker = format!("{MANAGED_BLOCK_END_PREFIX}{block_id}");

    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    let mut replaced = false;

    for line in existing.lines() {
        let trimmed = line.trim();
        if trimmed == start_marker {
            inside = true;
            if !replaced {
                out.push(start_marker.clone());
                for b in body.lines() {
                    out.push(b.to_string());
                }
                out.push(end_marker.clone());
                replaced = true;
            }
            continue;
        }
        if inside {
            if trimmed == end_marker {
                inside = false;
            }
            continue;
        }
        out.push(line.to_string());
    }

    if !replaced {
        if !out.is_empty() && !out.last().is_some_and(String::is_empty) {
            out.push(String::new());
        }
        out.push(start_marker);
        for b in body.lines() {
            out.push(b.to_string());
        }
        out.push(end_marker);
    }

    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

/// Read-only audit across all configured domains (guarantees zero mutations & zero backup dirs).
pub fn audit_system(ctx: &ReconcileContext, runner: &dyn CommandRunner) -> Result<ReconcileReport> {
    reconcile_system(ctx, ReconcileMode::Audit, runner)
}

/// Reconcile a dirty system across up to 9 domains in either `Audit` (read-only) or `Apply` mode.
pub fn reconcile_system(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    runner: &dyn CommandRunner,
) -> Result<ReconcileReport> {
    let domains = ctx.effective_domains();
    diag::log_event(
        LogLevel::Info,
        "debopty::reconcile",
        None,
        "starting reconcile run",
        None,
        &[
            ("mode", &format!("{mode:?}")),
            ("domains", &domains.len().to_string()),
            ("root", &ctx.root_prefix.to_string_lossy()),
        ],
    );

    let mut backup = BackupSession::new(ctx.root_prefix.clone(), ctx.backup_base_dir.clone());
    let mut findings: Vec<ReconcileFinding> = Vec::new();

    for domain in domains {
        match domain {
            ReconcileDomain::Ssh => {
                ssh::reconcile_ssh(ctx, mode, &mut backup, runner, &mut findings)?;
            }
            ReconcileDomain::Firewall => {
                firewall::reconcile_firewall(ctx, mode, &mut backup, runner, &mut findings)?;
            }
            ReconcileDomain::Dns => {
                system_net::reconcile_dns(ctx, mode, &mut backup, &mut findings)?;
            }
            ReconcileDomain::Sysctl => {
                system_net::reconcile_sysctl(ctx, mode, &mut backup, runner, &mut findings)?;
            }
            ReconcileDomain::Limits => {
                system_net::reconcile_limits(ctx, mode, &mut backup, &mut findings)?;
            }
            ReconcileDomain::Swap => {
                system_net::reconcile_swap(ctx, mode, &mut backup, runner, &mut findings)?;
            }
            ReconcileDomain::Logrotate => {
                system_net::reconcile_logrotate(ctx, mode, &mut backup, &mut findings)?;
            }
            ReconcileDomain::Apt => {
                system_net::reconcile_apt(ctx, mode, &mut backup, runner, &mut findings)?;
            }
            ReconcileDomain::Legacy => {
                system_net::reconcile_legacy(ctx, mode, &mut backup, &mut findings)?;
            }
        }
    }

    let drift_count = findings
        .iter()
        .filter(|f| {
            matches!(
                f.status,
                FindingStatus::WouldFix | FindingStatus::Fixed | FindingStatus::Quarantined
            )
        })
        .count();
    let changes_made = if mode == ReconcileMode::Audit {
        0
    } else {
        findings
            .iter()
            .filter(|f| matches!(f.status, FindingStatus::Fixed | FindingStatus::Quarantined))
            .count()
    };
    let preserved_count = findings
        .iter()
        .filter(|f| f.status == FindingStatus::PreservedUserCustom)
        .count();

    let backup_dir = if mode == ReconcileMode::Apply {
        backup.run_dir().map(PathBuf::from)
    } else {
        None
    };

    diag::log_event(
        LogLevel::Info,
        "debopty::reconcile",
        None,
        "finished reconcile run",
        None,
        &[
            ("drift_count", &drift_count.to_string()),
            ("changes_made", &changes_made.to_string()),
            ("preserved_count", &preserved_count.to_string()),
        ],
    );

    Ok(ReconcileReport {
        mode,
        backup_dir,
        findings,
        drift_count,
        changes_made,
        preserved_count,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use tempfile::TempDir;

    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};

    fn ok_out(stdout: &str) -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_out(stderr: &str) -> CmdOutput {
        CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: stderr.to_string(),
            timed_out: false,
        }
    }

    fn seed_dirty_nine_domain_rootfs(root: &std::path::Path) {
        // 0. /etc/passwd + /etc/debian_version + /etc/debopti/debopti.conf (corrupt + custom key/comment)
        fs::create_dir_all(root.join("etc/debopti")).unwrap();
        fs::create_dir_all(root.join("home/alice/.ssh")).unwrap();
        fs::write(
            root.join("etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();
        fs::write(root.join("etc/debian_version"), "12.8\n").unwrap();
        let corrupt_conf = b"\xEF\xBB\xBF# user custom header note\r\nSSH_HARDENED=\"true\"\r\nBASE_OPTIMIZED=\"true\"\r\nBASE_OPTIMIZED=\"true\"\r\nMY_CUSTOM_FLAG=\"keep-me\r\n\0";
        let conf_path = root.join("etc/debopti/debopti.conf");
        fs::write(&conf_path, corrupt_conf).unwrap();
        fs::set_permissions(&conf_path, fs::Permissions::from_mode(0o600)).unwrap();

        // 1. SSH (RW-SSH-01: cloud-init conflict + broken drop-in + socket port drift + custom Match/AllowUsers + CRLF/dup authorized_keys)
        fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
        fs::create_dir_all(root.join("etc/systemd/system/ssh.socket.d")).unwrap();
        let sshd_conf = root.join("etc/ssh/sshd_config");
        fs::write(
            &sshd_conf,
            "Port 22\nAllowUsers alice\nClientAliveInterval 120\nMatch User backup\n    AllowTcpForwarding no\n",
        )
        .unwrap();
        fs::set_permissions(&sshd_conf, fs::Permissions::from_mode(0o666)).unwrap();
        fs::write(
            root.join("etc/ssh/sshd_config.d/50-cloud-init.conf"),
            "PasswordAuthentication yes\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/ssh/sshd_config.d/90-broken.conf"),
            "BrokenDirectiveWithoutValue\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/ssh/sshd_config.d/60-user-custom.conf"),
            "ClientAliveCountMax 5\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/systemd/system/ssh.socket.d/override.conf"),
            "[Socket]\nListenStream=\nListenStream=52222\n",
        )
        .unwrap();
        let auth_keys = root.join("home/alice/.ssh/authorized_keys");
        fs::write(
            &auth_keys,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyOne11111111111111111111111111111111 alice@laptop\r\nssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyOne11111111111111111111111111111111 dup\r\nfrom=\"10.0.0.0/8\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIMockKeyTwo22222222222222222222222222222222 ops@jump\r\n",
        )
        .unwrap();
        fs::set_permissions(
            root.join("home/alice/.ssh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(&auth_keys, fs::Permissions::from_mode(0o644)).unwrap();

        // 2. Firewall + Fail2ban (RW-FW-01 netavark + global flush + UFW + legacy nft dir + user ignoreip)
        fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
        fs::create_dir_all(root.join("etc/nftables/debopti.d")).unwrap();
        fs::create_dir_all(root.join("etc/ufw")).unwrap();
        fs::create_dir_all(root.join("etc/fail2ban/jail.d")).unwrap();
        fs::create_dir_all(root.join("run/systemd/system")).unwrap();
        fs::write(
            root.join("etc/nftables.conf"),
            "#!/usr/sbin/nft -f\nflush ruleset\ntable inet netavark {\n    chain postrouting {}\n}\ntable inet filter {\n    chain input {\n        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept\n        tcp dport 22 accept comment \"SSH_Access_Port\"\n    }\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/nftables.d/70-user-custom.nft"),
            "table inet custom_user {\n    chain input {}\n}\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/nftables/debopti.d/legacy_web.nft"),
            "table inet filter {\n    chain input {\n        tcp dport 8443 accept comment \"legacy_web\"\n    }\n}\n",
        )
        .unwrap();
        fs::write(root.join("etc/ufw/ufw.conf"), "ENABLED=yes\n").unwrap();
        fs::write(
            root.join("etc/ufw/user.rules"),
            "-A ufw-user-input -p tcp --dport 8080 -j ACCEPT\n-A ufw-user-input -p udp --dport 51820 -j ACCEPT\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/fail2ban/jail.local"),
            "[DEFAULT]\nignoreip = 203.0.113.10\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/fail2ban/jail.d/99-debopty.local"),
            "[DEFAULT]\nignoreip = 127.0.0.1/8 ::1\nbackend = auto\n[sshd]\nlogpath = /var/log/auth.log\n",
        )
        .unwrap();

        // 3. DNS (dangling symlink)
        symlink(
            "/nonexistent/stub-resolv.conf",
            root.join("etc/resolv.conf"),
        )
        .unwrap();

        // 4. Sysctl (conflicting key + custom key + 0600)
        fs::create_dir_all(root.join("etc/sysctl.d")).unwrap();
        let custom_sysctl = root.join("etc/sysctl.d/80-user.conf");
        fs::write(
            &custom_sysctl,
            "net.ipv4.tcp_congestion_control = cubic\nvm.swappiness = 10\n",
        )
        .unwrap();
        fs::set_permissions(&custom_sysctl, fs::Permissions::from_mode(0o600)).unwrap();

        // 5. Limits (missing [Manager] header + custom systemd directive + user limits.d)
        fs::create_dir_all(root.join("etc/systemd")).unwrap();
        fs::create_dir_all(root.join("etc/security/limits.d")).unwrap();
        fs::write(
            root.join("etc/systemd/system.conf"),
            "DefaultTimeoutStopSec=15s\nDefaultLimitNOFILE=1048576\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/security/limits.d/50-user.conf"),
            "alice soft nproc 65535\n",
        )
        .unwrap();

        // 6. Swap (fstab CRLF + missing /swapfile + custom mount + broken zramswap)
        fs::write(
            root.join("etc/fstab"),
            "UUID=1111-2222 /data ext4 defaults 0 2\r\n/swapfile none swap sw 0 0\r\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("etc/default")).unwrap();
        fs::write(root.join("etc/default/zramswap"), "ALGO=\nPERCENT=999\r\n").unwrap();

        // 7. Logrotate (conflicting weekly + misplaced daily below include + custom dateext)
        fs::write(
            root.join("etc/logrotate.conf"),
            "weekly\ndateext\ninclude /etc/logrotate.d\ndaily\n",
        )
        .unwrap();

        // 8. APT (deb822 overlap + duplicate lines + 3rd party repo preserved)
        fs::create_dir_all(root.join("etc/apt/sources.list.d")).unwrap();
        fs::write(
            root.join("etc/apt/sources.list"),
            "deb https://deb.debian.org/debian bookworm main\ndeb https://deb.debian.org/debian bookworm main\ndeb https://custom.example.com/apt bookworm main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/debian.sources"),
            "Types: deb\nURIs: https://deb.debian.org/debian\nSuites: bookworm\nComponents: main\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/apt/sources.list.d/docker.list"),
            "deb https://download.docker.com/linux/debian bookworm stable\n",
        )
        .unwrap();

        // 9. Legacy /opt/debopti + legacy shortcuts + custom shell profile
        fs::create_dir_all(root.join("opt/debopti")).unwrap();
        fs::write(root.join("opt/debopti/user_note.txt"), "keep backup").unwrap();
        fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        fs::write(root.join("usr/local/bin/debopti"), "#!/bin/sh\n").unwrap();
        fs::create_dir_all(root.join("etc/profile.d")).unwrap();
        fs::write(root.join("etc/profile.d/debopti.sh"), "alias debopti=old\n").unwrap();
        fs::write(root.join("etc/profile.d/99-custom.sh"), "export FOO=bar\n").unwrap();
    }

    #[test]
    fn nine_domain_audit_apply_idempotency_u8_preservation_and_restore() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        seed_dirty_nine_domain_rootfs(root);

        let mut ctx = ReconcileContext::with_root(root);
        ctx.no_sys = true;
        ctx.f2b_backend_override = Some("systemd".to_string());
        let runner = MockCommandRunner::new();

        // 1. Audit is strictly read-only (0 changes, 0 backup dir)
        let audit1 = audit_system(&ctx, &runner).unwrap();
        assert_eq!(audit1.mode, ReconcileMode::Audit);
        assert_eq!(audit1.changes_made, 0);
        assert!(audit1.backup_dir.is_none());
        assert!(
            audit1.drift_count >= 15,
            "expected drift across all 9 domains, got {}",
            audit1.drift_count
        );
        assert!(audit1.preserved_count >= 10);
        assert!(!ctx.backup_base_dir.exists());

        // 2. Apply converges all 9 domains with atomic backup
        let apply1 = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert!(apply1.changes_made >= 15);
        assert!(apply1.preserved_count >= 10);
        let backup_dir = apply1.backup_dir.clone().expect("backup dir created");
        assert!(backup_dir.join("manifest.json").is_file());

        // Verify RW-SSH-01 + U8 SSH preservation
        assert!(!root.join("etc/ssh/sshd_config.d/90-broken.conf").exists());
        assert!(
            root.join("etc/ssh/sshd_config.d/90-broken.conf.disabled")
                .is_file()
        );
        let hardening =
            fs::read_to_string(root.join("etc/ssh/sshd_config.d/00-debopty-hardening.conf"))
                .unwrap();
        assert!(hardening.contains("# managed-by-debopty"));
        assert!(hardening.contains("PasswordAuthentication no"));
        let sshd_after = fs::read_to_string(root.join("etc/ssh/sshd_config")).unwrap();
        assert!(sshd_after.contains("Include /etc/ssh/sshd_config.d/*.conf"));
        assert!(sshd_after.contains("Port 52222"));
        assert!(sshd_after.contains("AllowUsers alice"));
        assert!(sshd_after.contains("Match User backup"));
        let auth_after = fs::read_to_string(root.join("home/alice/.ssh/authorized_keys")).unwrap();
        assert!(!auth_after.contains('\r'));
        assert_eq!(auth_after.lines().count(), 2);
        assert!(auth_after.contains("from=\"10.0.0.0/8\""));

        // Verify RW-FW-01 + U8 Firewall/Fail2ban preservation
        let nft_after = fs::read_to_string(root.join("etc/nftables.conf")).unwrap();
        assert!(!nft_after.contains("flush ruleset"));
        assert!(nft_after.contains("flush table inet filter"));
        assert!(nft_after.contains("table inet netavark"));
        assert!(nft_after.contains("nd-router-solicit"));
        assert!(nft_after.contains(r#"tcp dport 52222 accept comment "SSH_Access_Port""#));
        assert!(root.join("etc/nftables.d/70-user-custom.nft").is_file());
        let ufw_mig =
            fs::read_to_string(root.join("etc/nftables.d/debopti/90-ufw-migrated-ports.nft"))
                .unwrap();
        assert!(ufw_mig.contains("tcp dport 8080"));
        assert!(ufw_mig.contains("udp dport 51820"));
        assert!(
            fs::read_to_string(root.join("etc/ufw/ufw.conf"))
                .unwrap()
                .contains("ENABLED=no")
        );
        let f2b_after =
            fs::read_to_string(root.join("etc/fail2ban/jail.d/99-debopty.local")).unwrap();
        assert!(f2b_after.contains("203.0.113.10"));
        assert!(f2b_after.contains("backend = systemd"));
        assert!(root.join("etc/fail2ban/jail.local").is_file()); // custom jail.local preserved!

        // Verify DNS, Sysctl, Limits, Swap, Logrotate, APT, Legacy + U8 preservation
        assert!(
            fs::read_to_string(root.join("etc/resolv.conf"))
                .unwrap()
                .contains("# managed-by-debopty")
        );
        let user_sysctl = fs::read_to_string(root.join("etc/sysctl.d/80-user.conf")).unwrap();
        assert!(
            user_sysctl.contains(
                "# [debopty-reconcile] shadowed: net.ipv4.tcp_congestion_control = cubic"
            )
        );
        assert!(user_sysctl.contains("vm.swappiness = 10"));
        let sys_conf = fs::read_to_string(root.join("etc/systemd/system.conf")).unwrap();
        assert!(sys_conf.starts_with("[Manager]\n"));
        assert!(sys_conf.contains("DefaultTimeoutStopSec=15s"));
        let fstab_after = fs::read_to_string(root.join("etc/fstab")).unwrap();
        assert!(fstab_after.contains("UUID=1111-2222 /data ext4 defaults 0 2"));
        assert!(fstab_after.contains("# [debopty-reconcile] missing-swapfile:"));
        let lr_after = fs::read_to_string(root.join("etc/logrotate.conf")).unwrap();
        assert!(lr_after.contains("dateext"));
        assert!(
            lr_after.find("daily").unwrap() < lr_after.find("include /etc/logrotate.d").unwrap()
        );
        let apt_after = fs::read_to_string(root.join("etc/apt/sources.list")).unwrap();
        assert!(apt_after.contains("shadowed-by-debian.sources"));
        assert!(apt_after.contains("https://custom.example.com/apt"));
        assert!(root.join("etc/apt/sources.list.d/docker.list").is_file());
        let conf_after = fs::read_to_string(root.join("etc/debopti/debopti.conf")).unwrap();
        assert!(conf_after.contains("# user custom header note"));
        assert!(conf_after.contains("MY_CUSTOM_FLAG=\"keep-me\""));
        assert!(root.join("etc/profile.d/99-custom.sh").is_file());
        assert!(root.join("etc/profile.d/debopty.sh").is_file());
        assert!(!root.join("opt/debopti").exists());

        // 3. Idempotency: 2nd Apply yields changes_made == 0 & backup_dir == None; Audit yields drift_count == 0
        let apply2 = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap();
        assert_eq!(
            apply2.changes_made, 0,
            "2nd apply must be idempotent: {:?}",
            apply2.findings
        );
        assert_eq!(apply2.drift_count, 0);
        assert!(apply2.backup_dir.is_none());

        let audit2 = audit_system(&ctx, &runner).unwrap();
        assert_eq!(audit2.drift_count, 0);
        assert_eq!(audit2.changes_made, 0);

        // 4. One-command restore brings back original pre-reconcile artifacts cleanly
        let restored = restore_backup(&backup_dir, root).unwrap();
        assert!(restored >= 15);
        assert!(root.join("etc/ssh/sshd_config.d/90-broken.conf").is_file());
        assert!(
            !root
                .join("etc/ssh/sshd_config.d/90-broken.conf.disabled")
                .exists()
        );
        assert!(
            fs::read_to_string(root.join("etc/nftables.conf"))
                .unwrap()
                .contains("flush ruleset")
        );
        assert!(
            fs::symlink_metadata(root.join("etc/resolv.conf"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(root.join("opt/debopti/user_note.txt").is_file());
    }

    #[test]
    fn sshd_t_and_nft_validation_failures_auto_rollback_mutations() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("etc/ssh/sshd_config.d")).unwrap();
        fs::write(root.join("etc/ssh/sshd_config"), "Port 22\n").unwrap();
        fs::write(
            root.join("etc/ssh/sshd_config.d/90-bad.conf"),
            "OnlyOneToken\n",
        )
        .unwrap();

        let mut ctx = ReconcileContext::with_root(root);
        ctx.no_sys = false;
        ctx.domains = vec![ReconcileDomain::Ssh];

        let runner = MockCommandRunner::new();
        runner.on_exact("sshd", &["-t"], err_out("syntax error in sshd_config"));

        let err = reconcile_system(&ctx, ReconcileMode::Apply, &runner).unwrap_err();
        assert!(matches!(err, CoreError::Reconcile(_)));
        // Original broken drop-in was restored and .disabled removed!
        assert!(root.join("etc/ssh/sshd_config.d/90-bad.conf").is_file());
        assert!(
            !root
                .join("etc/ssh/sshd_config.d/90-bad.conf.disabled")
                .exists()
        );

        // Also test nft -c -f rollback
        fs::create_dir_all(root.join("etc/nftables.d")).unwrap();
        fs::write(
            root.join("etc/nftables.conf"),
            "flush ruleset\ntable inet netavark {}\ntable inet filter { chain input { tcp dport 22 accept comment \"SSH_Access_Port\" } }\n",
        )
        .unwrap();
        let mut fw_ctx = ReconcileContext::with_root(root);
        fw_ctx.no_sys = false;
        fw_ctx.domains = vec![ReconcileDomain::Firewall];
        let fw_runner = MockCommandRunner::new();
        fw_runner.on_exact("nft", &["list", "tables"], ok_out("table inet netavark\n"));
        fw_runner.on_exact("dpkg", &["-s", "ufw"], err_out("not installed"));
        fw_runner.on_prog("nft", err_out("nft check failed"));
        // Override list tables after on_prog so list tables still succeeds
        fw_runner.on_exact("nft", &["list", "tables"], ok_out("table inet netavark\n"));

        let fw_err = reconcile_system(&fw_ctx, ReconcileMode::Apply, &fw_runner).unwrap_err();
        assert!(matches!(fw_err, CoreError::Reconcile(_)));
        assert!(
            fs::read_to_string(root.join("etc/nftables.conf"))
                .unwrap()
                .contains("flush ruleset")
        );
    }

    #[test]
    fn managed_block_upsert_preserves_surrounding_user_lines_and_is_idempotent() {
        let orig = "# user header\nCUSTOM=1\n";
        let once = upsert_managed_block(orig, "net", "KEY_A=1\nKEY_B=2");
        assert!(once.contains("CUSTOM=1"));
        assert!(once.contains("# >>> debopty-managed:net"));
        assert!(once.contains("KEY_A=1"));
        let twice = upsert_managed_block(&once, "net", "KEY_A=1\nKEY_B=2");
        assert_eq!(once, twice);
        let updated = upsert_managed_block(&twice, "net", "KEY_A=9");
        assert!(updated.contains("CUSTOM=1"));
        assert!(updated.contains("KEY_A=9"));
        assert!(!updated.contains("KEY_B=2"));
    }
}
