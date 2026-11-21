use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::config::ConfigStore;
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::reconcile::backup::BackupSession;
use crate::reconcile::{
    FindingSeverity, FindingStatus, ReconcileContext, ReconcileDomain, ReconcileFinding,
    ReconcileMode,
};
use crate::sys::{self, CommandRunner};

const HARDENING_DROPIN_ABS: &str = "/etc/ssh/sshd_config.d/00-debopty-hardening.conf";
const SSHD_CONFIG_ABS: &str = "/etc/ssh/sshd_config";
const SSH_SOCKET_OVERRIDE_ABS: &str = "/etc/systemd/system/ssh.socket.d/override.conf";

fn sshd_match_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*match\s+").expect("valid sshd match re"))
}

fn sshd_include_conf_d_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*include\s+.*sshd_config\.d/\*\.conf").expect("valid sshd include re")
    })
}

fn sshd_port_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*port\s+([0-9]+)\b").expect("valid sshd port re"))
}

fn ssh_socket_listen_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*ListenStream\s*=(.*)$").expect("valid socket listen re"))
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..idx],
            _ => {}
        }
    }
    line
}

/// Detect clearly broken OpenSSH drop-in syntax (unbalanced quotes, missing value,
/// invalid keyword characters, invalid port, or garbage boolean for auth directives).
pub(crate) fn is_dropin_syntactically_broken(content: &str) -> bool {
    for raw in content.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Check unbalanced unescaped double quotes
        let mut quotes = 0usize;
        let mut prev_bs = false;
        for ch in trimmed.chars() {
            if ch == '"' && !prev_bs {
                quotes += 1;
            }
            prev_bs = ch == '\\' && !prev_bs;
        }
        if !quotes.is_multiple_of(2) {
            return true;
        }

        let code = strip_comment(trimmed).trim();
        if code.is_empty() {
            continue;
        }
        let mut parts = code.split_whitespace();
        let Some(keyword) = parts.next() else {
            continue;
        };
        let Some(first_val) = parts.next() else {
            // Every OpenSSH directive requires at least one argument
            return true;
        };
        let mut kw_chars = keyword.chars();
        let first_ch = kw_chars.next().unwrap_or('_');
        if !first_ch.is_ascii_alphabetic()
            || !kw_chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return true;
        }

        if keyword.eq_ignore_ascii_case("Port") {
            let Ok(p) = first_val.parse::<u16>() else {
                return true;
            };
            if p == 0 {
                return true;
            }
        } else if keyword.eq_ignore_ascii_case("PasswordAuthentication")
            || keyword.eq_ignore_ascii_case("PubkeyAuthentication")
        {
            if !first_val.eq_ignore_ascii_case("yes") && !first_val.eq_ignore_ascii_case("no") {
                return true;
            }
        } else if keyword.eq_ignore_ascii_case("PermitRootLogin") {
            let low = first_val.to_ascii_lowercase();
            if !matches!(
                low.as_str(),
                "yes" | "no" | "prohibit-password" | "without-password" | "forced-commands-only"
            ) {
                return true;
            }
        }
    }
    false
}

fn dropin_enables_weak_auth(content: &str) -> bool {
    for raw in content.lines() {
        let code = strip_comment(raw).trim();
        if code.is_empty() {
            continue;
        }
        if sshd_match_re().is_match(code) {
            break;
        }
        let mut parts = code.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(val) = parts.next() else {
            continue;
        };
        if (key.eq_ignore_ascii_case("PasswordAuthentication")
            || key.eq_ignore_ascii_case("PermitRootLogin"))
            && val.eq_ignore_ascii_case("yes")
        {
            return true;
        }
    }
    false
}

fn parse_socket_override_port(root: &Path) -> Option<u16> {
    let socket_d = crate::reconcile::backup::prefixed(root, "/etc/systemd/system/ssh.socket.d");
    let mut files: Vec<PathBuf> = fs::read_dir(&socket_d)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "conf"))
        .collect();
    files.sort();

    let mut last_port = None;
    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            if let Some(caps) = ssh_socket_listen_re().captures(line) {
                let val = caps[1].trim();
                if val.is_empty() {
                    last_port = None;
                } else {
                    let port_str = val.rsplit(':').next().unwrap_or(val);
                    if let Ok(p) = port_str.parse::<u16>()
                        && p >= 1
                    {
                        last_port = Some(p);
                    }
                }
            }
        }
    }
    last_port
}

fn parse_sshd_global_port(content: &str) -> Option<u16> {
    for raw in content.lines() {
        let code = strip_comment(raw);
        if sshd_match_re().is_match(code) {
            break;
        }
        if let Some(caps) = sshd_port_re().captures(code)
            && let Ok(p) = caps[1].parse::<u16>()
            && p >= 1
        {
            return Some(p);
        }
    }
    None
}

fn has_include_above_match_and_conflicts(content: &str) -> bool {
    let mut saw_include = false;
    for raw in content.lines() {
        let code = strip_comment(raw).trim();
        if code.is_empty() {
            continue;
        }
        if sshd_match_re().is_match(code) {
            break;
        }
        if sshd_include_conf_d_re().is_match(code) {
            saw_include = true;
            continue;
        }
        // If a conflicting weak auth line appears BEFORE Include, first-match-wins would let it win!
        if !saw_include {
            let mut parts = code.split_whitespace();
            if let (Some(k), Some(v)) = (parts.next(), parts.next())
                && (k.eq_ignore_ascii_case("PasswordAuthentication")
                    || k.eq_ignore_ascii_case("PermitRootLogin"))
                && v.eq_ignore_ascii_case("yes")
            {
                return false;
            }
        }
    }
    saw_include
}

fn ensure_include_at_top_of_sshd_config(content: &str) -> String {
    let include_line = "Include /etc/ssh/sshd_config.d/*.conf";
    let mut out: Vec<String> = Vec::new();
    let mut inserted = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if sshd_include_conf_d_re().is_match(trimmed) && !trimmed.starts_with('#') {
            if !inserted {
                out.push(include_line.to_string());
                inserted = true;
            }
            continue;
        }
        if !inserted && !trimmed.is_empty() && !trimmed.starts_with('#') {
            out.push(include_line.to_string());
            inserted = true;
        }
        out.push(line.to_string());
    }
    if !inserted {
        out.insert(0, include_line.to_string());
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

fn update_sshd_global_port(content: &str, new_port: u16) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let match_idx = lines
        .iter()
        .position(|l| sshd_match_re().is_match(strip_comment(l)))
        .unwrap_or(lines.len());

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 1);
    let mut updated = false;
    for &line in &lines[..match_idx] {
        let code = strip_comment(line);
        if sshd_port_re().is_match(code) {
            if !updated {
                out.push(format!("Port {new_port}"));
                updated = true;
            }
        } else {
            out.push(line.to_string());
        }
    }
    if !updated {
        out.push(format!("Port {new_port}"));
    }
    for &line in &lines[match_idx..] {
        out.push(line.to_string());
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

fn extract_ssh_key_identity(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    for i in 0..tokens.len() {
        let tok = tokens[i];
        if (tok.starts_with("ssh-")
            || tok.starts_with("ecdsa-sha2-")
            || tok.starts_with("sk-ssh-")
            || tok.starts_with("sk-ecdsa-"))
            && i + 1 < tokens.len()
        {
            return Some((tok.to_string(), tokens[i + 1].to_string()));
        }
    }
    None
}

fn has_custom_sshd_directives(content: &str) -> bool {
    let custom_keys = [
        "allowusers",
        "allowgroups",
        "denyusers",
        "clientaliveinterval",
        "clientalivecountmax",
        "subsystem",
        "maxauthtries",
        "listenaddress",
        "banner",
    ];
    for raw in content.lines() {
        let code = strip_comment(raw).trim();
        if code.is_empty() {
            continue;
        }
        if sshd_match_re().is_match(code) {
            return true;
        }
        if let Some(key) = code.split_whitespace().next()
            && custom_keys.contains(&key.to_ascii_lowercase().as_str())
        {
            return true;
        }
    }
    false
}

pub(crate) fn reconcile_ssh(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    runner: &dyn CommandRunner,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::ssh",
        None,
        "reconciling SSH domain",
        None,
        &[("mode", &format!("{mode:?}"))],
    );

    let cfg = ConfigStore::new(&ctx.config_path)
        .load()
        .unwrap_or_default();
    let engine = ctx.template_engine();
    let mut mutated_paths: Vec<String> = Vec::new();

    let sshd_conf_path = ctx.path(SSHD_CONFIG_ABS);
    let conf_d_path = ctx.path("/etc/ssh/sshd_config.d");

    // 1. Repair insecure permissions on /etc/ssh/sshd_config
    if sshd_conf_path.is_file()
        && let Ok(meta) = fs::metadata(&sshd_conf_path)
    {
        let file_mode = meta.permissions().mode() & 0o777;
        if (file_mode & 0o022) != 0 {
            let fixed_mode = file_mode & !0o022;
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(SSHD_CONFIG_ABS)?;
                fs::set_permissions(&sshd_conf_path, fs::Permissions::from_mode(fixed_mode))?;
                mutated_paths.push(SSHD_CONFIG_ABS.to_string());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: "SSH_INSECURE_CONFIG_PERMS".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: SSHD_CONFIG_ABS.to_string(),
                summary: format!(
                    "sshd_config 权限过宽 ({file_mode:04o})，应收敛为 {fixed_mode:04o}"
                ),
                backup_rel_path: bak,
            });
        }
    }

    // 2. Scan /etc/ssh/sshd_config.d/*.conf for perms, syntax errors, conflicts, and U8 custom drop-ins
    let mut has_conflicting_dropin = false;
    let mut custom_dropins: Vec<String> = Vec::new();
    if conf_d_path.is_dir() {
        let mut files: Vec<PathBuf> = fs::read_dir(&conf_d_path)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "conf"))
            .collect();
        files.sort();

        for file in files {
            let Some(fname) = file.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let logical = format!("/etc/ssh/sshd_config.d/{fname}");
            let content = fs::read_to_string(&file).unwrap_or_default();

            if is_dropin_syntactically_broken(&content) {
                let disabled_logical = format!("{logical}.disabled");
                let disabled_path = ctx.path(&disabled_logical);
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(&logical)?;
                    let _ = backup.snapshot_path(&disabled_logical)?;
                    fs::rename(&file, &disabled_path)?;
                    mutated_paths.push(logical.clone());
                    mutated_paths.push(disabled_logical);
                    (FindingStatus::Quarantined, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Ssh,
                    code: "SSH_BROKEN_DROPIN".to_string(),
                    severity: FindingSeverity::Critical,
                    status,
                    target: logical,
                    summary: format!(
                        "检测到语法损坏的 SSH drop-in ({fname})，隔离至 .disabled 以防 sshd 失联"
                    ),
                    backup_rel_path: bak,
                });
                continue;
            }

            // Permissions check on valid drop-ins
            if let Ok(meta) = fs::metadata(&file) {
                let file_mode = meta.permissions().mode() & 0o777;
                if file_mode != 0o644 {
                    let (status, bak) = if mode == ReconcileMode::Apply {
                        let b = backup.snapshot_path(&logical)?;
                        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))?;
                        mutated_paths.push(logical.clone());
                        (FindingStatus::Fixed, b)
                    } else {
                        (FindingStatus::WouldFix, None)
                    };
                    findings.push(ReconcileFinding {
                        domain: ReconcileDomain::Ssh,
                        code: "SSH_INSECURE_DROPIN_PERMS".to_string(),
                        severity: FindingSeverity::Warning,
                        status,
                        target: logical.clone(),
                        summary: format!("SSH drop-in 权限异常 ({file_mode:04o})，修正为 0644"),
                        backup_rel_path: bak,
                    });
                }
            }

            if fname != "00-debopty-hardening.conf" {
                if dropin_enables_weak_auth(&content) {
                    has_conflicting_dropin = true;
                } else {
                    custom_dropins.push(logical);
                }
            }
        }
    }

    for custom in custom_dropins {
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Ssh,
            code: "SSH_CUSTOM_DROPIN_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: custom,
            summary: "保留合法的用户自定义 SSH drop-in 配置".to_string(),
            backup_rel_path: None,
        });
    }

    // 3. Conflicting drop-in resolution (RW-SSH-01: e.g. 50-cloud-init.conf when SSH_HARDENED=true)
    let hardening_path = ctx.path(HARDENING_DROPIN_ABS);
    if cfg.ssh_hardened || hardening_path.is_file() {
        let mut vars = BTreeMap::new();
        vars.insert("PERMIT_ROOT_LOGIN".to_string(), "no".to_string());
        vars.insert("PASSWORD_AUTH".to_string(), "no".to_string());
        let expected_hardening =
            engine.render_to_string("templates/security/00-debopty-hardening.conf", &vars)?;

        let current_hardening = fs::read_to_string(&hardening_path).unwrap_or_default();
        let dropin_needs_sync =
            !hardening_path.is_file() || current_hardening != expected_hardening;

        let sshd_needs_include = sshd_conf_path.is_file()
            && !has_include_above_match_and_conflicts(
                &fs::read_to_string(&sshd_conf_path).unwrap_or_default(),
            );

        if dropin_needs_sync || sshd_needs_include {
            let code = if has_conflicting_dropin {
                "SSH_CONFLICTING_DROPIN"
            } else {
                "SSH_HARDENING_DROPIN_SYNC"
            };
            let (status, bak) = if mode == ReconcileMode::Apply {
                let mut first_bak = None;
                if dropin_needs_sync {
                    first_bak = backup.snapshot_path(HARDENING_DROPIN_ABS)?;
                    sys::write_file_atomic(
                        &hardening_path,
                        expected_hardening.as_bytes(),
                        Some(0o644),
                    )?;
                    mutated_paths.push(HARDENING_DROPIN_ABS.to_string());
                }
                if sshd_needs_include {
                    let b = backup.snapshot_path(SSHD_CONFIG_ABS)?;
                    if first_bak.is_none() {
                        first_bak = b;
                    }
                    let orig = fs::read_to_string(&sshd_conf_path)?;
                    let next = ensure_include_at_top_of_sshd_config(&orig);
                    sys::write_file_atomic(&sshd_conf_path, next.as_bytes(), None)?;
                    mutated_paths.push(SSHD_CONFIG_ABS.to_string());
                }
                (FindingStatus::Fixed, first_bak)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: code.to_string(),
                severity: if has_conflicting_dropin {
                    FindingSeverity::Critical
                } else {
                    FindingSeverity::Warning
                },
                status,
                target: HARDENING_DROPIN_ABS.to_string(),
                summary: if has_conflicting_dropin {
                    "通过 00-debopty-hardening.conf 与顶部 Include 覆盖 cloud-init/第三方弱认证冲突"
                        .to_string()
                } else {
                    "同步 00-debopty-hardening.conf 模板并确保 sshd_config 顶部 Include 生效"
                        .to_string()
                },
                backup_rel_path: bak,
            });
        }
    }

    // 4. Detect & align ssh.socket.d/override.conf vs sshd_config Port drift (RW-SSH-01)
    let socket_port = parse_socket_override_port(&ctx.root_prefix);
    let sshd_content = fs::read_to_string(&sshd_conf_path).unwrap_or_default();
    let sshd_port = parse_sshd_global_port(&sshd_content);

    if let Some(sp) = socket_port {
        if let Some(cp) = sshd_port
            && cp != sp
            && sshd_conf_path.is_file()
        {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(SSHD_CONFIG_ABS)?;
                let updated = update_sshd_global_port(&sshd_content, sp);
                sys::write_file_atomic(&sshd_conf_path, updated.as_bytes(), None)?;
                mutated_paths.push(SSHD_CONFIG_ABS.to_string());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: "SSH_SOCKET_PORT_DRIFT".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: SSHD_CONFIG_ABS.to_string(),
                summary: format!(
                    "ssh.socket 端口 ({sp}) 与 sshd_config ({cp}) 漂移，已对齐至 {sp}"
                ),
                backup_rel_path: bak,
            });
        }
        if sp != 22 {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: "SSH_CUSTOM_PORT_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: SSH_SOCKET_OVERRIDE_ABS.to_string(),
                summary: format!("保留用户自定义 SSH 端口 {sp}"),
                backup_rel_path: None,
            });
        }
    } else if let Some(cp) = sshd_port
        && cp != 22
    {
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Ssh,
            code: "SSH_CUSTOM_PORT_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: SSHD_CONFIG_ABS.to_string(),
            summary: format!("保留用户自定义 SSH 端口 {cp}"),
            backup_rel_path: None,
        });
    }

    // 5. Preserve custom directives & Match blocks in sshd_config (U8)
    if sshd_conf_path.is_file() && has_custom_sshd_directives(&sshd_content) {
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Ssh,
            code: "SSH_CUSTOM_DIRECTIVES_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: SSHD_CONFIG_ABS.to_string(),
            summary: "保留 sshd_config 中的 Match 块与自定义用户指令".to_string(),
            backup_rel_path: None,
        });
    }

    // 6. Real users' ~/.ssh (0700) and authorized_keys (0600, CRLF, dedupe, preserve all distinct keys)
    let passwd_path = ctx.path("/etc/passwd");
    let passwd_entries = fs::read_to_string(&passwd_path)
        .map(|t| sys::parse_passwd(&t))
        .unwrap_or_default();
    let mut user_homes: Vec<(String, PathBuf)> = Vec::new();
    let real_users = sys::all_real_users_with_dir_check(&passwd_entries, |home| {
        ctx.path(&home.to_string_lossy()).is_dir()
    });
    for uname in real_users {
        let logical_home = passwd_entries
            .iter()
            .find(|e| e.name == uname)
            .map(|e| e.home.clone())
            .unwrap_or_else(|| {
                if uname == "root" {
                    PathBuf::from("/root")
                } else {
                    PathBuf::from(format!("/home/{uname}"))
                }
            });
        user_homes.push((uname, logical_home));
    }

    for (_uname, logical_home) in user_homes {
        let ssh_dir_logical = format!(
            "{}/.ssh",
            logical_home.to_string_lossy().trim_end_matches('/')
        );
        let ssh_dir = ctx.path(&ssh_dir_logical);
        if ssh_dir.is_dir()
            && let Ok(meta) = fs::metadata(&ssh_dir)
        {
            let dir_mode = meta.permissions().mode() & 0o777;
            if dir_mode != 0o700 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(&ssh_dir_logical)?;
                    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))?;
                    mutated_paths.push(ssh_dir_logical.clone());
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Ssh,
                    code: "SSH_DIR_PERMS".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: ssh_dir_logical.clone(),
                    summary: format!(".ssh 目录权限异常 ({dir_mode:04o})，收敛为 0700"),
                    backup_rel_path: bak,
                });
            }
        }

        let auth_logical = format!("{ssh_dir_logical}/authorized_keys");
        let auth_path = ctx.path(&auth_logical);
        if !auth_path.is_file() {
            continue;
        }

        if let Ok(meta) = fs::metadata(&auth_path) {
            let file_mode = meta.permissions().mode() & 0o777;
            if file_mode != 0o600 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(&auth_logical)?;
                    fs::set_permissions(&auth_path, fs::Permissions::from_mode(0o600))?;
                    mutated_paths.push(auth_logical.clone());
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Ssh,
                    code: "SSH_AUTH_KEYS_PERMS".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: auth_logical.clone(),
                    summary: format!("authorized_keys 权限异常 ({file_mode:04o})，收敛为 0600"),
                    backup_rel_path: bak,
                });
            }
        }

        let raw = fs::read_to_string(&auth_path).unwrap_or_default();
        let has_crlf = raw.contains('\r');
        let mut seen_keys: BTreeSet<(String, String)> = BTreeSet::new();
        let mut out_lines: Vec<String> = Vec::new();
        let mut dup_removed = 0usize;
        let mut key_count = 0usize;

        for raw_line in raw.lines() {
            let clean = raw_line.trim_end_matches('\r');
            if let Some(identity) = extract_ssh_key_identity(clean) {
                if seen_keys.insert(identity) {
                    out_lines.push(clean.to_string());
                    key_count += 1;
                } else {
                    dup_removed += 1;
                }
            } else {
                out_lines.push(clean.to_string());
            }
        }

        if has_crlf || dup_removed > 0 {
            let mut normalized = out_lines.join("\n");
            if !normalized.is_empty() {
                normalized.push('\n');
            }
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(&auth_logical)?;
                sys::write_file_atomic(&auth_path, normalized.as_bytes(), Some(0o600))?;
                mutated_paths.push(auth_logical.clone());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: "SSH_AUTH_KEYS_NORMALIZED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: auth_logical.clone(),
                summary: format!(
                    "规范化 authorized_keys（CRLF 修复={}，去重 {} 条，保留 {} 条独立公钥）",
                    has_crlf, dup_removed, key_count
                ),
                backup_rel_path: bak,
            });
        }

        if key_count > 0 {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Ssh,
                code: "SSH_CUSTOM_AUTH_KEYS_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: auth_logical,
                summary: format!("完整保留 {key_count} 条用户 SSH 公钥及其选项/注释"),
                backup_rel_path: None,
            });
        }
    }

    // 7. Validate `sshd -t` in live Apply mode; auto-rollback SSH snapshots on failure
    if mode == ReconcileMode::Apply && !ctx.no_sys && !mutated_paths.is_empty() {
        let check = runner.run("sshd", &["-t"], Duration::from_secs(10))?;
        if !check.success() {
            let _ = backup.restore_paths(&mutated_paths);
            diag::log_event(
                LogLevel::Error,
                "debopty::reconcile::ssh",
                Some("E5002"),
                "sshd -t failed after SSH reconcile; rolled back SSH mutations",
                Some("请检查 /etc/ssh/sshd_config 与 drop-in 语法"),
                &[("stderr", check.stderr.trim())],
            );
            return Err(CoreError::Reconcile(format!(
                "sshd -t validation failed (SSH changes auto-rolled back): {}",
                check.stderr.trim()
            )));
        }
    }

    Ok(())
}
