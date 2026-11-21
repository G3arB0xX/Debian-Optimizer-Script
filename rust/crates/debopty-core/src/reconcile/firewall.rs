use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::reconcile::backup::BackupSession;
use crate::reconcile::{
    FindingSeverity, FindingStatus, ReconcileContext, ReconcileDomain, ReconcileFinding,
    ReconcileMode,
};
use crate::security::f2b_sync::{self, F2bSyncOptions};
use crate::sys::{self, CommandRunner};

const NFT_CONF_ABS: &str = "/etc/nftables.conf";
const NFT_MOD_ENTRY_ABS: &str = "/etc/nftables.d/50-debopti.nft";
const UFW_CONF_ABS: &str = "/etc/ufw/ufw.conf";
const UFW_MIGRATED_NFT_ABS: &str = "/etc/nftables.d/debopti/90-ufw-migrated-ports.nft";
const F2B_JAIL_LOCAL_ABS: &str = "/etc/fail2ban/jail.local";
const F2B_DEBOPTY_JAIL_ABS: &str = "/etc/fail2ban/jail.d/99-debopty.local";
const F2B_PORTS_JAIL_ABS: &str = "/etc/fail2ban/jail.d/99-debopty-ports.local";
const F2B_GUARD_NFT_ABS: &str = "/etc/nftables.d/55-debopty-f2b-guard.nft";

const LEGACY_JAIL_SIGNATURES: &[&str] = &[
    "banaction = nftables-multiport",
    "banaction_allports = nftables-allports",
    "logpath = /var/log/auth.log",
    "bantime = 86400",
    "findtime = 600",
];

fn nft_table_decl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^\s*table\s+(inet|ip|ip6|bridge|arp|netdev)\s+([A-Za-z0-9_.-]+)")
            .expect("valid nft table decl regex")
    })
}

fn flush_ruleset_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*flush\s+ruleset\s*;?\s*$").expect("valid flush ruleset re"))
}

fn ssh_access_rule_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"tcp\s+dport\s+(?:\{[^}]*\}|[0-9,\s\-]+)\s+accept\s+comment\s+"SSH_Access_Port""#,
        )
        .expect("valid ssh access rule regex")
    })
}

fn ufw_rule_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"-A\s+ufw6?-user-input\b.*?-p\s+(tcp|udp)\b.*?(?:--dport|--dports)\s+([0-9,:]+)\b.*?-j\s+ACCEPT",
        )
        .expect("valid ufw rule regex")
    })
}

fn ignoreip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*ignoreip\s*=\s*(.+)$").expect("valid ignoreip regex"))
}

fn strip_hash_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn is_external_nft_table(family: &str, name: &str) -> bool {
    !(family == "inet" && (name == "filter" || name == "debopty_f2b_guard"))
}

fn collect_external_tables(ctx: &ReconcileContext, runner: &dyn CommandRunner) -> BTreeSet<String> {
    let mut tables = BTreeSet::new();

    // 1. Scan /etc/nftables.conf
    if let Ok(content) = fs::read_to_string(ctx.path(NFT_CONF_ABS)) {
        for line in content.lines() {
            let code = strip_hash_comment(line);
            if let Some(caps) = nft_table_decl_re().captures(code)
                && is_external_nft_table(&caps[1], &caps[2])
            {
                tables.insert(format!("{} {}", &caps[1], &caps[2]));
            }
        }
    }

    // 2. Scan /etc/nftables.d/*.nft
    let nft_d = ctx.path("/etc/nftables.d");
    if let Ok(entries) = fs::read_dir(&nft_d) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "nft")
                && let Ok(content) = fs::read_to_string(&path)
            {
                for line in content.lines() {
                    let code = strip_hash_comment(line);
                    if let Some(caps) = nft_table_decl_re().captures(code)
                        && is_external_nft_table(&caps[1], &caps[2])
                    {
                        tables.insert(format!("{} {}", &caps[1], &caps[2]));
                    }
                }
            }
        }
    }

    // 3. Live `nft list tables` (when !no_sys)
    if !ctx.no_sys
        && let Ok(out) = runner.run("nft", &["list", "tables"], Duration::from_secs(5))
        && out.success()
    {
        for line in out.stdout.lines() {
            if let Some(caps) = nft_table_decl_re().captures(line)
                && is_external_nft_table(&caps[1], &caps[2])
            {
                tables.insert(format!("{} {}", &caps[1], &caps[2]));
            }
        }
    }

    tables
}

fn has_global_flush_ruleset(content: &str) -> bool {
    content
        .lines()
        .any(|line| flush_ruleset_re().is_match(strip_hash_comment(line)))
}

fn scope_global_flush_ruleset(content: &str) -> String {
    let mut out = Vec::new();
    for line in content.lines() {
        if flush_ruleset_re().is_match(strip_hash_comment(line)) {
            out.push("table inet filter {}".to_string());
            out.push("flush table inet filter".to_string());
        } else {
            out.push(line.to_string());
        }
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

fn parse_ufw_allowed_ports(
    ctx: &ReconcileContext,
    exclude_ssh: u16,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut tcp = BTreeSet::new();
    let mut udp = BTreeSet::new();

    for abs in ["/etc/ufw/user.rules", "/etc/ufw/user6.rules"] {
        let path = ctx.path(abs);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        for line in content.lines() {
            if line.trim_start().starts_with('#') {
                continue;
            }
            if let Some(caps) = ufw_rule_re().captures(line) {
                let proto = &caps[1];
                let raw_ports = &caps[2];
                for tok in raw_ports.split(',') {
                    let trimmed = tok.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(p) = trimmed.parse::<u16>() {
                        if p == 0 || p == exclude_ssh {
                            continue;
                        }
                        if proto == "tcp" {
                            tcp.insert(p.to_string());
                        } else {
                            udp.insert(p.to_string());
                        }
                    } else if let Some((a, b)) = trimmed.split_once(':')
                        && let (Ok(p1), Ok(p2)) = (a.parse::<u16>(), b.parse::<u16>())
                        && p1 >= 1
                        && p1 <= p2
                    {
                        let expr = format!("{p1} - {p2}");
                        if proto == "tcp" {
                            tcp.insert(expr);
                        } else {
                            udp.insert(expr);
                        }
                    }
                }
            }
        }
    }

    (tcp, udp)
}

fn format_nft_port_set(items: &BTreeSet<String>) -> String {
    let joined: Vec<&str> = items.iter().map(String::as_str).collect();
    if joined.len() == 1 && !joined[0].contains(" - ") {
        joined[0].to_string()
    } else {
        format!("{{ {} }}", joined.join(", "))
    }
}

fn ufw_conf_is_enabled(content: &str) -> bool {
    for line in content.lines() {
        let code = strip_hash_comment(line).trim();
        if let Some((k, v)) = code.split_once('=')
            && k.trim().eq_ignore_ascii_case("ENABLED")
            && v.trim().eq_ignore_ascii_case("yes")
        {
            return true;
        }
    }
    false
}

fn disable_ufw_conf(content: &str) -> String {
    let mut out = Vec::new();
    let mut replaced = false;
    for line in content.lines() {
        let code = strip_hash_comment(line).trim();
        if let Some((k, _)) = code.split_once('=')
            && k.trim().eq_ignore_ascii_case("ENABLED")
        {
            out.push("ENABLED=no".to_string());
            replaced = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !replaced {
        out.push("ENABLED=no".to_string());
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

fn extract_ignoreip_tokens(content: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(caps) = ignoreip_re().captures(trimmed) {
            for tok in caps[1].split(|c: char| c.is_whitespace() || c == ',') {
                let t = tok.trim();
                if !t.is_empty() && !t.starts_with('#') {
                    tokens.push(t.to_string());
                }
            }
        }
    }
    tokens
}

fn is_legacy_f2b_jail_local(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().map(|l| l.trim_end_matches('\r')).collect();
    lines.len() <= 15 && LEGACY_JAIL_SIGNATURES.iter().all(|sig| lines.contains(sig))
}

fn merge_ignoreip_in_jail(content: &str, merged_line: &str) -> String {
    let mut out = Vec::new();
    let mut updated = false;
    let mut in_default = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_default = trimmed.eq_ignore_ascii_case("[DEFAULT]");
            out.push(line.to_string());
            continue;
        }
        if in_default && ignoreip_re().is_match(trimmed) {
            if !updated {
                out.push(merged_line.to_string());
                updated = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !updated {
        // Insert under [DEFAULT] or at top
        if let Some(pos) = out
            .iter()
            .position(|l| l.trim().eq_ignore_ascii_case("[DEFAULT]"))
        {
            out.insert(pos + 1, merged_line.to_string());
        } else {
            out.insert(0, merged_line.to_string());
            out.insert(0, "[DEFAULT]".to_string());
        }
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

fn switch_f2b_to_systemd_backend(content: &str) -> (String, bool) {
    let mut out = Vec::new();
    let mut changed = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("logpath = /var/log/auth.log")
            || trimmed.eq_ignore_ascii_case("backend = auto")
        {
            out.push("backend = systemd".to_string());
            changed = true;
        } else {
            out.push(line.to_string());
        }
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    (rendered, changed)
}

fn f2b_sync_has_drift(
    ctx: &ReconcileContext,
    opts: &F2bSyncOptions,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    let engine = ctx.template_engine();
    let probe_opts = F2bSyncOptions {
        print_ports: true,
        ..opts.clone()
    };
    let report = f2b_sync::run_f2b_sync(&probe_opts, &engine, runner)?;
    let has_ports = !report.all_public_ports_f2b.is_empty();

    let ports_path = ctx.path(F2B_PORTS_JAIL_ABS);
    let guard_path = ctx.path(F2B_GUARD_NFT_ABS);

    if !ports_path.is_file() {
        return Ok(true);
    }
    let ports_txt = fs::read_to_string(&ports_path).unwrap_or_default();
    if !ports_txt.contains(&format!("port = {}", report.ssh_port)) {
        return Ok(true);
    }
    let expected_ports_val = if has_ports {
        report.all_public_ports_f2b.as_str()
    } else {
        ""
    };
    if has_ports && !ports_txt.contains(&format!("port = {expected_ports_val}")) {
        return Ok(true);
    }
    if has_ports && !guard_path.is_file() {
        return Ok(true);
    }
    if !has_ports && guard_path.is_file() {
        return Ok(true);
    }
    if has_ports && guard_path.is_file() {
        let guard_txt = fs::read_to_string(&guard_path).unwrap_or_default();
        if !report.tcp_ports_nft.is_empty()
            && !guard_txt.contains(&format!("tcp dport {}", report.tcp_ports_nft))
        {
            return Ok(true);
        }
        if !report.udp_ports_nft.is_empty()
            && !guard_txt.contains(&format!("udp dport {}", report.udp_ports_nft))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn reconcile_firewall(
    ctx: &ReconcileContext,
    mode: ReconcileMode,
    backup: &mut BackupSession,
    runner: &dyn CommandRunner,
    findings: &mut Vec<ReconcileFinding>,
) -> Result<()> {
    diag::log_event(
        LogLevel::Debug,
        "debopty::reconcile::firewall",
        None,
        "reconciling Firewall and Fail2ban domain",
        None,
        &[("mode", &format!("{mode:?}"))],
    );

    let engine = ctx.template_engine();
    let mut mutated_nft_paths: Vec<String> = Vec::new();
    let active_ssh_port =
        f2b_sync::detect_ssh_port_from_configs(&ctx.root_prefix, ctx.no_sys, runner);

    // 1. Detect external / container nft tables (RW-FW-01: netavark, docker, ip nat, custom tables)
    let external_tables = collect_external_tables(ctx, runner);
    if !external_tables.is_empty() {
        let list_str = external_tables
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Firewall,
            code: "FW_CONTAINER_TABLES_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: NFT_CONF_ABS.to_string(),
            summary: format!("检测到外部/容器 nftables 表 ({list_str})，全程受保护不清空"),
            backup_rel_path: None,
        });
    }

    // 2. Replace global `flush ruleset` with table-scoped flush when external/container tables exist (RW-FW-01)
    let nft_conf_path = ctx.path(NFT_CONF_ABS);
    if nft_conf_path.is_file() && !external_tables.is_empty() {
        let content = fs::read_to_string(&nft_conf_path)?;
        if has_global_flush_ruleset(&content) {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(NFT_CONF_ABS)?;
                let scoped = scope_global_flush_ruleset(&content);
                sys::write_file_atomic(&nft_conf_path, scoped.as_bytes(), Some(0o644))?;
                mutated_nft_paths.push(NFT_CONF_ABS.to_string());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "FW_GLOBAL_FLUSH_SCOPED".to_string(),
                severity: FindingSeverity::Critical,
                status,
                target: NFT_CONF_ABS.to_string(),
                summary: "将全局 `flush ruleset` 替换为表级刷新 `flush table inet filter`，防止清空 Docker/Podman netavark 规则链".to_string(),
                backup_rel_path: bak,
            });
        }
    }

    // 3. Detect UFW & migrate allowed TCP/UDP ports via rule_template.nft before disabling
    let ufw_conf_path = ctx.path(UFW_CONF_ABS);
    let ufw_conf_enabled = ufw_conf_path.is_file()
        && ufw_conf_is_enabled(&fs::read_to_string(&ufw_conf_path).unwrap_or_default());
    let ufw_pkg_installed = !ctx.no_sys
        && runner
            .run("dpkg", &["-s", "ufw"], Duration::from_secs(5))
            .map(|o| o.success())
            .unwrap_or(false);

    if ufw_conf_enabled || ufw_pkg_installed {
        let (tcp_ports, udp_ports) = parse_ufw_allowed_ports(ctx, active_ssh_port);
        if !tcp_ports.is_empty() || !udp_ports.is_empty() {
            let mut rule_lines = Vec::new();
            if !tcp_ports.is_empty() {
                rule_lines.push(format!(
                    "        tcp dport {} accept comment \"UFW_Migrated\"",
                    format_nft_port_set(&tcp_ports)
                ));
            }
            if !udp_ports.is_empty() {
                rule_lines.push(format!(
                    "        udp dport {} accept comment \"UFW_Migrated\"",
                    format_nft_port_set(&udp_ports)
                ));
            }
            let mut vars = BTreeMap::new();
            vars.insert("RULES".to_string(), rule_lines.join("\n"));
            let mut rendered =
                engine.render_to_string("templates/security/rule_template.nft", &vars)?;
            if !rendered.ends_with('\n') {
                rendered.push('\n');
            }

            let migrated_path = ctx.path(UFW_MIGRATED_NFT_ABS);
            let existing = fs::read_to_string(&migrated_path).unwrap_or_default();
            if existing != rendered {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(UFW_MIGRATED_NFT_ABS)?;
                    sys::write_file_atomic(&migrated_path, rendered.as_bytes(), Some(0o644))?;
                    mutated_nft_paths.push(UFW_MIGRATED_NFT_ABS.to_string());
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Firewall,
                    code: "FW_UFW_PORTS_MIGRATED".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: UFW_MIGRATED_NFT_ABS.to_string(),
                    summary: "迁移 UFW 放行端口规则至 nftables 模块化文件（保留业务连通性）"
                        .to_string(),
                    backup_rel_path: bak,
                });
            }
        }

        let (status, bak) = if mode == ReconcileMode::Apply {
            let bak_rel = if ufw_conf_path.is_file() {
                let b = backup.snapshot_path(UFW_CONF_ABS)?;
                let content = fs::read_to_string(&ufw_conf_path)?;
                let disabled = disable_ufw_conf(&content);
                sys::write_file_atomic(&ufw_conf_path, disabled.as_bytes(), Some(0o644))?;
                b
            } else {
                None
            };
            if !ctx.no_sys {
                let _ = runner.run("ufw", &["disable"], Duration::from_secs(15));
                if ufw_pkg_installed {
                    let _ = runner.run_with_env(
                        "apt-get",
                        &["purge", "-yq", "ufw"],
                        &[("DEBIAN_FRONTEND", "noninteractive")],
                        Duration::from_secs(120),
                    );
                }
            }
            (FindingStatus::Fixed, bak_rel)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Firewall,
            code: "FW_UFW_DISABLED".to_string(),
            severity: FindingSeverity::Warning,
            status,
            target: UFW_CONF_ABS.to_string(),
            summary: "检测到与 nftables 冲突的 UFW 防火墙，已迁移端口后停用".to_string(),
            backup_rel_path: bak,
        });
    }

    // 4. Migrate legacy /etc/nftables/debopti.d/*.nft safely (never delete unrelated /etc/nftables/*)
    let legacy_nft_root = ctx.path("/etc/nftables");
    let legacy_nft_dir = legacy_nft_root.join("debopti.d");
    if legacy_nft_dir.is_dir() {
        let (status, bak) = if mode == ReconcileMode::Apply {
            let mut first_bak = None;
            let dest_dir = ctx.path("/etc/nftables.d/debopti");
            fs::create_dir_all(&dest_dir)?;
            let mut entries: Vec<PathBuf> = fs::read_dir(&legacy_nft_dir)?
                .flatten()
                .map(|e| e.path())
                .collect();
            entries.sort();
            for p in entries {
                if p.is_file()
                    && let Some(fname) = p.file_name().and_then(|s| s.to_str())
                {
                    let old_logical = format!("/etc/nftables/debopti.d/{fname}");
                    let b = backup.snapshot_path(&old_logical)?;
                    if first_bak.is_none() {
                        first_bak = b;
                    }
                    if p.extension().is_some_and(|e| e == "nft") {
                        let new_logical = format!("/etc/nftables.d/debopti/{fname}");
                        let dest = dest_dir.join(fname);
                        if !dest.exists() {
                            let _ = backup.snapshot_path(&new_logical)?;
                            let bytes = fs::read(&p)?;
                            sys::write_file_atomic(&dest, &bytes, Some(0o644))?;
                            mutated_nft_paths.push(new_logical);
                        }
                    }
                }
            }
            if nft_conf_path.is_file() {
                let raw = fs::read_to_string(&nft_conf_path)?;
                let filtered: Vec<&str> = raw
                    .lines()
                    .filter(|l| !l.contains("/etc/nftables/debopti.d/"))
                    .collect();
                let mut next = filtered.join("\n");
                if !next.is_empty() {
                    next.push('\n');
                }
                if next != raw {
                    let _ = backup.snapshot_path(NFT_CONF_ABS)?;
                    sys::write_file_atomic(&nft_conf_path, next.as_bytes(), Some(0o644))?;
                    mutated_nft_paths.push(NFT_CONF_ABS.to_string());
                }
            }
            fs::remove_dir_all(&legacy_nft_dir)?;
            let _ = fs::remove_dir(&legacy_nft_root); // Only succeeds if empty!
            (FindingStatus::Fixed, first_bak)
        } else {
            (FindingStatus::WouldFix, None)
        };
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Firewall,
            code: "FW_LEGACY_NFT_DIR".to_string(),
            severity: FindingSeverity::Warning,
            status,
            target: "/etc/nftables/debopti.d".to_string(),
            summary: "迁移旧版 /etc/nftables/debopti.d 规则至 /etc/nftables.d/debopti".to_string(),
            backup_rel_path: bak,
        });
    }

    // 5. Reconcile /etc/nftables.conf (0644, NDP, active SSH port, include, 50-debopti.nft)
    if nft_conf_path.is_file() {
        if let Ok(meta) = fs::metadata(&nft_conf_path) {
            let file_mode = meta.permissions().mode() & 0o777;
            if (file_mode & 0o004) == 0 || (file_mode & 0o022) != 0 {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(NFT_CONF_ABS)?;
                    fs::set_permissions(&nft_conf_path, fs::Permissions::from_mode(0o644))?;
                    mutated_nft_paths.push(NFT_CONF_ABS.to_string());
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Firewall,
                    code: "FW_NFT_CONF_PERMS".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: NFT_CONF_ABS.to_string(),
                    summary: format!("nftables.conf 权限异常 ({file_mode:04o})，修正为 0644"),
                    backup_rel_path: bak,
                });
            }
        }

        let content = fs::read_to_string(&nft_conf_path)?;
        let mut updated = content.clone();
        let mut ndp_needed = false;
        let mut ssh_port_drift = false;
        let mut include_needed = false;

        if updated.contains("SSH_Access_Port")
            && updated.contains("icmpv6 type echo-request")
            && !updated.contains("nd-router-solicit")
        {
            ndp_needed = true;
            let mut lines = Vec::new();
            for line in updated.lines() {
                lines.push(line.to_string());
                if line.contains("icmpv6 type echo-request") {
                    lines.push(
                        "        # 核心：放行 IPv6 邻居发现协议 (NDP)，防止 IPv6 断网失联"
                            .to_string(),
                    );
                    lines.push(
                        "        ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } accept"
                            .to_string(),
                    );
                }
            }
            updated = lines.join("\n") + "\n";
        }

        let expected_ssh_rule =
            format!(r#"tcp dport {active_ssh_port} accept comment "SSH_Access_Port""#);
        if updated.contains(r#""SSH_Access_Port""#) && ssh_access_rule_re().is_match(&updated) {
            let replaced = ssh_access_rule_re()
                .replace_all(&updated, expected_ssh_rule.as_str())
                .into_owned();
            if replaced != updated {
                ssh_port_drift = true;
                updated = replaced;
            }
        }

        let has_nft_d_include = updated
            .lines()
            .any(|l| !l.trim_start().starts_with('#') && l.contains("nftables.d/*.nft"));
        if !has_nft_d_include {
            include_needed = true;
            if !updated.ends_with('\n') {
                updated.push('\n');
            }
            updated.push_str("include \"/etc/nftables.d/*.nft\"\n");
        }

        if ndp_needed || ssh_port_drift || include_needed {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(NFT_CONF_ABS)?;
                sys::write_file_atomic(&nft_conf_path, updated.as_bytes(), Some(0o644))?;
                mutated_nft_paths.push(NFT_CONF_ABS.to_string());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "FW_NFT_CONF_RECONCILED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: NFT_CONF_ABS.to_string(),
                summary: format!(
                    "校准 nftables.conf（NDP={}, SSH端口={}, include={}）",
                    ndp_needed, ssh_port_drift, include_needed
                ),
                backup_rel_path: bak,
            });
        }

        let mod_entry = ctx.path(NFT_MOD_ENTRY_ABS);
        if !mod_entry.is_file() {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(NFT_MOD_ENTRY_ABS)?;
                let debopti_dir = ctx.path("/etc/nftables.d/debopti");
                fs::create_dir_all(&debopti_dir)?;
                let mod_content = if ctx.root_prefix == Path::new("/") {
                    "include \"/etc/nftables.d/debopti/*.nft\"\n".to_string()
                } else {
                    format!("include \"{}/*.nft\"\n", debopti_dir.display())
                };
                sys::write_file_atomic(&mod_entry, mod_content.as_bytes(), Some(0o644))?;
                mutated_nft_paths.push(NFT_MOD_ENTRY_ABS.to_string());
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "FW_MOD_ENTRY_SYNC".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: NFT_MOD_ENTRY_ABS.to_string(),
                summary: "补齐 /etc/nftables.d/50-debopti.nft 模块规则入口".to_string(),
                backup_rel_path: bak,
            });
        }
    }

    // Preserve user custom .nft files in /etc/nftables.d/ outside debopty-managed files
    let nft_d = ctx.path("/etc/nftables.d");
    if nft_d.is_dir() {
        let mut custom_nft: Vec<String> = Vec::new();
        if let Ok(entries) = fs::read_dir(&nft_d) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file()
                    && p.extension().is_some_and(|e| e == "nft")
                    && let Some(name) = p.file_name().and_then(|s| s.to_str())
                    && name != "50-debopti.nft"
                    && name != "55-debopty-f2b-guard.nft"
                {
                    custom_nft.push(format!("/etc/nftables.d/{name}"));
                }
            }
        }
        custom_nft.sort();
        for c in custom_nft {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "FW_CUSTOM_NFT_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: c,
                summary: "保留用户自定义 nftables 片段文件".to_string(),
                backup_rel_path: None,
            });
        }
    }

    // Validate nft -c -f before nft -f when !no_sys and nft files mutated
    if mode == ReconcileMode::Apply
        && !ctx.no_sys
        && !mutated_nft_paths.is_empty()
        && nft_conf_path.is_file()
    {
        let conf_str = nft_conf_path.to_string_lossy().into_owned();
        let check = runner.run("nft", &["-c", "-f", &conf_str], Duration::from_secs(15))?;
        if !check.success() {
            let _ = backup.restore_paths(&mutated_nft_paths);
            diag::log_event(
                LogLevel::Error,
                "debopty::reconcile::firewall",
                Some("E5002"),
                "nft -c -f validation failed; rolled back firewall mutations",
                Some("请检查 /etc/nftables.conf 与片段规则语法"),
                &[("stderr", check.stderr.trim())],
            );
            return Err(CoreError::Reconcile(format!(
                "nft -c -f validation failed (firewall changes auto-rolled back): {}",
                check.stderr.trim()
            )));
        }
        let load = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15))?;
        if !load.success() {
            let _ = backup.restore_paths(&mutated_nft_paths);
            return Err(CoreError::Reconcile(format!(
                "nft -f load failed (firewall changes auto-rolled back): {}",
                load.stderr.trim()
            )));
        }
    }

    // 6. Fail2ban reconciliation (RW-SSH-01 ignoreip preservation, legacy jail cleanup, systemd backend, f2b_sync)
    let mut merged_ignoreip: Vec<String> = vec!["127.0.0.1/8".to_string(), "::1".to_string()];
    let mut user_custom_ignoreips: Vec<String> = Vec::new();
    let mut add_ignoreip = |tok: String| {
        if !merged_ignoreip.iter().any(|existing| existing == &tok) {
            merged_ignoreip.push(tok.clone());
            user_custom_ignoreips.push(tok);
        }
    };

    let jail_local_path = ctx.path(F2B_JAIL_LOCAL_ABS);
    if jail_local_path.is_file() {
        let content = fs::read_to_string(&jail_local_path).unwrap_or_default();
        for tok in extract_ignoreip_tokens(&content) {
            add_ignoreip(tok);
        }
        if is_legacy_f2b_jail_local(&content) {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(F2B_JAIL_LOCAL_ABS)?;
                fs::remove_file(&jail_local_path)?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "F2B_LEGACY_JAIL_REMOVED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: F2B_JAIL_LOCAL_ABS.to_string(),
                summary: "清理旧版 12 行自动生成的 /etc/fail2ban/jail.local".to_string(),
                backup_rel_path: bak,
            });
        } else {
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "F2B_CUSTOM_JAIL_LOCAL_PRESERVED".to_string(),
                severity: FindingSeverity::Info,
                status: FindingStatus::PreservedUserCustom,
                target: F2B_JAIL_LOCAL_ABS.to_string(),
                summary: "保留用户自定义的 /etc/fail2ban/jail.local".to_string(),
                backup_rel_path: None,
            });
        }
    }

    let jail_d_path = ctx.path("/etc/fail2ban/jail.d");
    if jail_d_path.is_dir() {
        let mut jail_files: Vec<PathBuf> = fs::read_dir(&jail_d_path)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "local"))
            .collect();
        jail_files.sort();

        for jf in jail_files {
            let Some(fname) = jf.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            let content = fs::read_to_string(&jf).unwrap_or_default();
            for tok in extract_ignoreip_tokens(&content) {
                add_ignoreip(tok);
            }
            if fname != "99-debopty.local" && fname != "99-debopty-ports.local" {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Firewall,
                    code: "F2B_CUSTOM_JAIL_D_PRESERVED".to_string(),
                    severity: FindingSeverity::Info,
                    status: FindingStatus::PreservedUserCustom,
                    target: format!("/etc/fail2ban/jail.d/{fname}"),
                    summary: format!("保留第三方/用户 Fail2ban 规则文件 ({fname})"),
                    backup_rel_path: None,
                });
            }
        }
    }

    if !user_custom_ignoreips.is_empty() {
        findings.push(ReconcileFinding {
            domain: ReconcileDomain::Firewall,
            code: "F2B_CUSTOM_IGNOREIP_PRESERVED".to_string(),
            severity: FindingSeverity::Info,
            status: FindingStatus::PreservedUserCustom,
            target: F2B_DEBOPTY_JAIL_ABS.to_string(),
            summary: format!(
                "保留运维白名单 ignoreip ({})，防止误封",
                user_custom_ignoreips.join(", ")
            ),
            backup_rel_path: None,
        });
    }

    let debopty_jail_path = ctx.path(F2B_DEBOPTY_JAIL_ABS);
    if debopty_jail_path.is_file() {
        let content = fs::read_to_string(&debopty_jail_path)?;
        let current_tokens = extract_ignoreip_tokens(&content);
        let missing_ignoreip = merged_ignoreip
            .iter()
            .any(|tok| !current_tokens.iter().any(|c| c == tok));

        if missing_ignoreip {
            let merged_line = format!("ignoreip = {}", merged_ignoreip.join(" "));
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b = backup.snapshot_path(F2B_DEBOPTY_JAIL_ABS)?;
                let next = merge_ignoreip_in_jail(&content, &merged_line);
                sys::write_file_atomic(&debopty_jail_path, next.as_bytes(), Some(0o644))?;
                (FindingStatus::Fixed, b)
            } else {
                (FindingStatus::WouldFix, None)
            };
            findings.push(ReconcileFinding {
                domain: ReconcileDomain::Firewall,
                code: "F2B_IGNOREIP_MERGED".to_string(),
                severity: FindingSeverity::Warning,
                status,
                target: F2B_DEBOPTY_JAIL_ABS.to_string(),
                summary: format!("已将用户白名单合并至 99-debopty.local ({merged_line})"),
                backup_rel_path: bak,
            });
        }

        // Switch to backend = systemd when systemd is present without rsyslog (or override=systemd)
        let prefer_systemd = ctx.f2b_backend_override.as_deref() == Some("systemd")
            || (ctx.path("/run/systemd/system").is_dir()
                && !ctx.path("/usr/sbin/rsyslogd").exists());
        if prefer_systemd {
            let latest = fs::read_to_string(&debopty_jail_path)?;
            let (rewritten, changed) = switch_f2b_to_systemd_backend(&latest);
            if changed {
                let (status, bak) = if mode == ReconcileMode::Apply {
                    let b = backup.snapshot_path(F2B_DEBOPTY_JAIL_ABS)?;
                    sys::write_file_atomic(&debopty_jail_path, rewritten.as_bytes(), Some(0o644))?;
                    (FindingStatus::Fixed, b)
                } else {
                    (FindingStatus::WouldFix, None)
                };
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Firewall,
                    code: "F2B_BACKEND_SYSTEMD".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: F2B_DEBOPTY_JAIL_ABS.to_string(),
                    summary: "无 rsyslog 的 systemd 环境切换 Fail2ban 至 backend = systemd"
                        .to_string(),
                    backup_rel_path: bak,
                });
            }
        }
    }

    // Sync Fail2ban public ports ONLY when drift is detected
    if debopty_jail_path.is_file() || nft_conf_path.is_file() {
        let sync_opts = F2bSyncOptions {
            root_prefix: ctx.root_prefix.clone(),
            no_sys: ctx.no_sys,
            force_reload: false,
            print_ports: false,
            backend_override: ctx.f2b_backend_override.clone(),
            template_dir: ctx.template_dir.clone(),
            ss_file: None,
        };
        if f2b_sync_has_drift(ctx, &sync_opts, runner)? {
            let (status, bak) = if mode == ReconcileMode::Apply {
                let b1 = backup.snapshot_path(F2B_PORTS_JAIL_ABS)?;
                let b2 = backup.snapshot_path(F2B_GUARD_NFT_ABS)?;
                let rep = f2b_sync::run_f2b_sync(&sync_opts, &engine, runner)?;
                if rep.guard_changed || rep.jail_changed {
                    (FindingStatus::Fixed, b1.or(b2))
                } else {
                    (FindingStatus::Clean, None)
                }
            } else {
                (FindingStatus::WouldFix, None)
            };
            if status != FindingStatus::Clean {
                findings.push(ReconcileFinding {
                    domain: ReconcileDomain::Firewall,
                    code: "F2B_PORT_SYNC".to_string(),
                    severity: FindingSeverity::Warning,
                    status,
                    target: F2B_PORTS_JAIL_ABS.to_string(),
                    summary: "同步 Fail2ban 公网端口防护与 nftables 端口规则".to_string(),
                    backup_rel_path: bak,
                });
            }
        }
    }

    Ok(())
}
