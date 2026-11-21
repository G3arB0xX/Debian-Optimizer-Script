use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::error::Result;
use crate::sys::{self, CommandRunner};
use crate::template::TemplateEngine;

#[derive(Debug, Clone)]
pub struct F2bSyncOptions {
    pub root_prefix: PathBuf,
    pub no_sys: bool,
    pub force_reload: bool,
    pub print_ports: bool,
    pub backend_override: Option<String>,
    pub template_dir: Option<PathBuf>,
    pub ss_file: Option<PathBuf>,
}

impl Default for F2bSyncOptions {
    fn default() -> Self {
        Self {
            root_prefix: PathBuf::from("/"),
            no_sys: false,
            force_reload: false,
            print_ports: false,
            backend_override: None,
            template_dir: None,
            ss_file: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct F2bSyncReport {
    pub ssh_port: u16,
    pub tcp_ports_nft: String,
    pub udp_ports_nft: String,
    pub all_public_ports_f2b: String,
    pub guard_changed: bool,
    pub jail_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortSpec {
    Single(u16),
    Range(u16, u16),
}

impl PortSpec {
    fn sort_key(&self) -> (u16, u8, u16) {
        match *self {
            PortSpec::Single(p) => (p, 0, p),
            PortSpec::Range(a, b) => (a, 1, b),
        }
    }

    fn format_nft(&self) -> String {
        match *self {
            PortSpec::Single(p) => p.to_string(),
            PortSpec::Range(a, b) => format!("{a} - {b}"),
        }
    }

    fn format_f2b(&self) -> String {
        match *self {
            PortSpec::Single(p) => p.to_string(),
            PortSpec::Range(a, b) => format!("{a}:{b}"),
        }
    }
}

impl Ord for PortSpec {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for PortSpec {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn under_root(root: &Path, rel: &str) -> PathBuf {
    let clean = rel.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

fn strip_hash_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn ssh_nft_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"tcp[ \t]+dport[ \t]+\{?[ \t]*([0-9]+)").expect("valid ssh nft regex")
    })
}

fn ssh_socket_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[ \t]*ListenStream[ \t]*=(.*)$").expect("valid socket regex"))
}

fn sshd_port_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^[ \t]*port[ \t]+([0-9]+)").expect("valid sshd port regex"))
}

fn sshd_match_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^[ \t]*match[ \t]+").expect("valid sshd match regex"))
}

fn parse_ssh_socket_dir_port(root_prefix: &Path) -> Option<u16> {
    let socket_d = under_root(root_prefix, "etc/systemd/system/ssh.socket.d");
    let mut conf_files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&socket_d) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "conf") {
                conf_files.push(path);
            }
        }
    }
    conf_files.sort();

    let mut last_port: Option<u16> = None;
    for file in conf_files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') || trimmed.starts_with(';') {
                continue;
            }
            if let Some(caps) = ssh_socket_re().captures(line) {
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

fn parse_sshd_config_file_port(content: &str) -> Option<u16> {
    for raw_line in content.lines() {
        let code = strip_hash_comment(raw_line);
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

fn parse_sshd_config_dir_port(root_prefix: &Path) -> Option<u16> {
    let conf_d = under_root(root_prefix, "etc/ssh/sshd_config.d");
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(&conf_d) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().is_some_and(|e| e == "conf") {
                files.push(path);
            }
        }
    }
    files.sort();
    for file in files {
        if let Ok(content) = fs::read_to_string(&file)
            && let Some(port) = parse_sshd_config_file_port(&content)
        {
            return Some(port);
        }
    }
    None
}

/// Detect configured SSH listening port across `ssh.socket.d/*.conf`, `sshd_config.d/*.conf`,
/// `sshd_config`, and `sshd -T`, defaulting to `22`.
pub(crate) fn detect_ssh_port_from_configs(
    root_prefix: &Path,
    no_sys: bool,
    runner: &dyn CommandRunner,
) -> u16 {
    // 1. systemd socket activation overrides (Debian 12/13 default ssh.socket)
    if let Some(port) = parse_ssh_socket_dir_port(root_prefix) {
        return port;
    }

    // 2. sshd_config.d/*.conf (first-match-wins when included at top of sshd_config)
    if let Some(port) = parse_sshd_config_dir_port(root_prefix) {
        return port;
    }

    // 3. /etc/ssh/sshd_config global Port directive (case-insensitive, above Match blocks)
    let sshd_conf = under_root(root_prefix, "etc/ssh/sshd_config");
    if let Ok(content) = fs::read_to_string(&sshd_conf)
        && let Some(port) = parse_sshd_config_file_port(&content)
    {
        return port;
    }

    // 4. Live sshd -T fallback (only when !no_sys)
    if !no_sys
        && let Ok(out) = runner.run("sshd", &["-T"], Duration::from_secs(5))
        && out.success()
    {
        for line in out.stdout.lines() {
            if let Some(rest) = line.strip_prefix("port ")
                && let Some(tok) = rest.split_whitespace().next()
                && let Ok(p) = tok.parse::<u16>()
                && p >= 1
            {
                return p;
            }
        }
    }

    22
}

fn policy_drop_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"policy[ \t]+drop").expect("valid policy drop regex"))
}

fn accept_word_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(^|[^A-Za-z0-9_])accept([^A-Za-z0-9_]|$)").expect("valid accept word regex")
    })
}

fn dport_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(tcp|udp)[ \t]+dport[ \t]+(\{[^}]*\}|[0-9]+[ \t]*[-:][ \t]*[0-9]+|[0-9]+)")
            .expect("valid dport regex")
    })
}

fn range_item_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^([0-9]+)[ \t]*[-:][ \t]*([0-9]+)$").expect("valid range item regex")
    })
}

fn extract_ssh_port(opts: &F2bSyncOptions, runner: &dyn CommandRunner) -> u16 {
    let nft_conf = under_root(&opts.root_prefix, "etc/nftables.conf");

    // 1. nftables.conf SSH_Access_Port comment line
    if let Ok(content) = fs::read_to_string(&nft_conf) {
        for line in content.lines() {
            if line.contains("SSH_Access_Port") {
                let code = strip_hash_comment(line);
                if let Some(caps) = ssh_nft_re().captures(code)
                    && let Ok(p) = caps[1].parse::<u16>()
                    && p >= 1
                {
                    return p;
                }
            }
        }
    }

    // 2-5. ssh.socket.d -> sshd_config.d -> sshd_config -> sshd -T -> 22
    detect_ssh_port_from_configs(&opts.root_prefix, opts.no_sys, runner)
}

fn nft_has_policy_drop(root_prefix: &Path) -> bool {
    let nft_conf = under_root(root_prefix, "etc/nftables.conf");
    let Ok(content) = fs::read_to_string(nft_conf) else {
        return false;
    };
    content
        .lines()
        .any(|line| policy_drop_re().is_match(strip_hash_comment(line)))
}

fn parse_nft_files(root_prefix: &Path) -> Vec<(Proto, PortSpec)> {
    let mut files: Vec<PathBuf> = Vec::new();
    let nft_conf = under_root(root_prefix, "etc/nftables.conf");
    if nft_conf.is_file() {
        files.push(nft_conf);
    }

    let nft_d = under_root(root_prefix, "etc/nftables.d");
    if nft_d.is_dir() {
        let mut sub_files: Vec<PathBuf> = WalkDir::new(&nft_d)
            .follow_links(false)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .filter(|e| {
                e.file_type().is_file()
                    && e.path().extension().is_some_and(|ext| ext == "nft")
                    && e.file_name() != "55-debopty-f2b-guard.nft"
            })
            .map(|e| e.into_path())
            .collect();
        sub_files.sort();
        files.extend(sub_files);
    }

    let mut results = Vec::new();
    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        for raw_line in content.lines() {
            let line = strip_hash_comment(raw_line);
            if !accept_word_re().is_match(line) {
                continue;
            }
            for caps in dport_re().captures_iter(line) {
                let proto = if &caps[1] == "tcp" {
                    Proto::Tcp
                } else {
                    Proto::Udp
                };
                let expr = caps[2].replace(['{', '}'], "");
                for item in expr.split(',') {
                    let trimmed = item.trim();
                    if let Ok(p) = trimmed.parse::<u16>() {
                        if p >= 1 {
                            results.push((proto, PortSpec::Single(p)));
                        }
                    } else if let Some(rcaps) = range_item_re().captures(trimmed)
                        && let (Ok(p1), Ok(p2)) = (rcaps[1].parse::<u16>(), rcaps[2].parse::<u16>())
                        && p1 >= 1
                        && p1 <= p2
                    {
                        results.push((proto, PortSpec::Range(p1, p2)));
                    }
                }
            }
        }
    }
    results
}

fn parse_ss_public_ports(
    opts: &F2bSyncOptions,
    runner: &dyn CommandRunner,
) -> Vec<(Proto, PortSpec)> {
    let ss_data = if let Some(ref path) = opts.ss_file {
        fs::read_to_string(path).unwrap_or_default()
    } else if !opts.no_sys {
        runner
            .run("ss", &["-tulnH"], Duration::from_secs(5))
            .map(|o| if o.success() { o.stdout } else { String::new() })
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut results = Vec::new();
    for line in ss_data.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        let netid = fields[0];
        let local_addr = fields[4];
        let proto = if netid.starts_with("tcp") {
            Proto::Tcp
        } else if netid.starts_with("udp") {
            Proto::Udp
        } else {
            continue;
        };

        let Some((raw_host, port_str)) = local_addr.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port_str.parse::<u16>() else {
            continue;
        };
        if port == 0 {
            continue;
        }

        let host = raw_host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .split('%')
            .next()
            .unwrap_or("");
        let host_lower = host.to_ascii_lowercase();
        if host_lower.starts_with("127.")
            || host_lower == "::1"
            || host_lower.starts_with("169.254.")
            || host_lower.starts_with("fe80:")
        {
            continue;
        }

        results.push((proto, PortSpec::Single(port)));
    }
    results
}

fn format_nft_port_expr(items: &BTreeSet<PortSpec>) -> String {
    if items.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = items.iter().map(PortSpec::format_nft).collect();
    let joined = parts.join(", ");
    if items.len() > 1 || joined.contains(" - ") {
        format!("{{ {joined} }}")
    } else {
        joined
    }
}

fn format_f2b_port_list(items: &BTreeSet<PortSpec>) -> String {
    items
        .iter()
        .map(PortSpec::format_f2b)
        .collect::<Vec<_>>()
        .join(",")
}

fn detect_public_guard_backend(opts: &F2bSyncOptions, runner: &dyn CommandRunner) -> String {
    if let Some(ref b) = opts.backend_override
        && !b.is_empty()
    {
        return if b == "systemd" {
            "backend = systemd\njournalmatch = _TRANSPORT=kernel".to_string()
        } else {
            "logpath = /var/log/kern.log".to_string()
        };
    }

    if under_root(&opts.root_prefix, "run/systemd/system").is_dir() {
        let py_ok = runner
            .run(
                "python3",
                &["-c", "import systemd.journal"],
                Duration::from_secs(3),
            )
            .map(|o| o.success())
            .unwrap_or(false);
        let dpkg_ok = if py_ok {
            true
        } else {
            sys::is_dpkg_installed("python3-systemd", runner)
        };
        if dpkg_ok {
            return "backend = systemd\njournalmatch = _TRANSPORT=kernel".to_string();
        }
    }

    "logpath = /var/log/kern.log".to_string()
}

fn write_if_changed(path: &Path, content: &[u8], mode: u32) -> Result<bool> {
    if let Ok(existing) = fs::read(path)
        && existing == content
    {
        return Ok(false);
    }

    sys::write_file_atomic(path, content, Some(mode))?;
    Ok(true)
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

/// Native Rust port of `templates/security/debopty-f2b-sync.sh`.
pub fn run_f2b_sync(
    opts: &F2bSyncOptions,
    templates: &TemplateEngine,
    runner: &dyn CommandRunner,
) -> Result<F2bSyncReport> {
    let ssh_port = extract_ssh_port(opts, runner);

    let mut discovered = parse_nft_files(&opts.root_prefix);
    if !nft_has_policy_drop(&opts.root_prefix) {
        discovered.extend(parse_ss_public_ports(opts, runner));
    }

    let mut tcp_ports = BTreeSet::new();
    let mut udp_ports = BTreeSet::new();
    let mut all_ports = BTreeSet::new();

    for (proto, spec) in discovered {
        match proto {
            Proto::Tcp => {
                tcp_ports.insert(spec);
            }
            Proto::Udp => {
                udp_ports.insert(spec);
            }
        }
        all_ports.insert(spec);
    }

    let tcp_ports_nft = format_nft_port_expr(&tcp_ports);
    let udp_ports_nft = format_nft_port_expr(&udp_ports);
    let all_public_ports_f2b = format_f2b_port_list(&all_ports);

    if opts.print_ports {
        return Ok(F2bSyncReport {
            ssh_port,
            tcp_ports_nft,
            udp_ports_nft,
            all_public_ports_f2b,
            guard_changed: false,
            jail_changed: false,
        });
    }

    let effective_engine;
    let engine = if let Some(ref dir) = opts.template_dir {
        effective_engine = TemplateEngine::new(Some(dir.clone()));
        &effective_engine
    } else {
        templates
    };

    let has_ports = !all_ports.is_empty();
    let public_guard_enabled = if has_ports { "true" } else { "false" };
    let f2b_ports_value = if has_ports {
        all_public_ports_f2b.clone()
    } else {
        ssh_port.to_string()
    };

    let mut guard_rules = String::new();
    if has_ports {
        if !tcp_ports_nft.is_empty() {
            guard_rules.push_str(&format!(
                "        ct state new tcp dport {tcp_ports_nft} update @abuse_v4 {{ ip saddr limit rate over 60/minute burst 30 packets }} log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second\n"
            ));
            guard_rules.push_str(&format!(
                "        ct state new tcp dport {tcp_ports_nft} update @abuse_v6 {{ ip6 saddr limit rate over 60/minute burst 30 packets }} log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
            ));
        }
        if !udp_ports_nft.is_empty() {
            if !guard_rules.is_empty() {
                guard_rules.push('\n');
            }
            guard_rules.push_str(&format!(
                "        ct state new udp dport {udp_ports_nft} update @abuse_v4 {{ ip saddr limit rate over 60/minute burst 30 packets }} log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second\n"
            ));
            guard_rules.push_str(&format!(
                "        ct state new udp dport {udp_ports_nft} update @abuse_v6 {{ ip6 saddr limit rate over 60/minute burst 30 packets }} log prefix \"[DEBOPTY-F2B-PORT-ABUSE] \" level warn limit rate 5/second"
            ));
        }
    }

    let public_guard_backend = detect_public_guard_backend(opts, runner);

    let mut jail_vars = BTreeMap::new();
    jail_vars.insert("SSH_PORT".to_string(), ssh_port.to_string());
    jail_vars.insert(
        "PUBLIC_GUARD_ENABLED".to_string(),
        public_guard_enabled.to_string(),
    );
    jail_vars.insert("ALL_PUBLIC_PORTS_F2B".to_string(), f2b_ports_value);
    jail_vars.insert("PUBLIC_GUARD_BACKEND".to_string(), public_guard_backend);

    let rendered_jail = ensure_trailing_newline(
        engine.render_to_string("templates/security/fail2ban_ports.local", &jail_vars)?,
    );

    let is_root = sys::current_euid() == 0;
    let mut guard_valid = true;
    let mut rendered_guard = String::new();

    if has_ports {
        let mut guard_vars = BTreeMap::new();
        guard_vars.insert("GUARD_RULES".to_string(), guard_rules);
        rendered_guard = ensure_trailing_newline(
            engine
                .render_to_string("templates/security/f2b_public_ports_guard.nft", &guard_vars)?,
        );

        if !opts.no_sys && is_root {
            let mut tmp_check = NamedTempFile::new()?;
            tmp_check.write_all(rendered_guard.as_bytes())?;
            tmp_check.flush()?;
            if let Some(p) = tmp_check.path().to_str()
                && let Ok(out) = runner.run("nft", &["-c", "-f", p], Duration::from_secs(10))
                && !out.success()
            {
                guard_valid = false;
            }
        }
    }

    let nft_guard_file = under_root(&opts.root_prefix, "etc/nftables.d/55-debopty-f2b-guard.nft");
    let f2b_ports_file = under_root(
        &opts.root_prefix,
        "etc/fail2ban/jail.d/99-debopty-ports.local",
    );

    let mut guard_changed = false;
    if has_ports {
        if guard_valid {
            guard_changed = write_if_changed(&nft_guard_file, rendered_guard.as_bytes(), 0o644)?;
        }
    } else if nft_guard_file.is_file() {
        fs::remove_file(&nft_guard_file)?;
        guard_changed = true;
    }

    let jail_changed = write_if_changed(&f2b_ports_file, rendered_jail.as_bytes(), 0o644)?;

    if !opts.no_sys && is_root && (guard_changed || jail_changed || opts.force_reload) {
        if guard_changed || opts.force_reload {
            if has_ports && guard_valid {
                if let Some(p) = nft_guard_file.to_str() {
                    let _ = runner.run("nft", &["-f", p], Duration::from_secs(10));
                }
            } else if !has_ports {
                let _ = runner.run(
                    "nft",
                    &["delete", "table", "inet", "debopty_f2b_guard"],
                    Duration::from_secs(10),
                );
            }
        }

        if jail_changed || opts.force_reload {
            let f2b_active = runner
                .run(
                    "systemctl",
                    &["is-active", "--quiet", "fail2ban"],
                    Duration::from_secs(5),
                )
                .map(|o| o.success())
                .unwrap_or(false);
            if f2b_active {
                let reloaded = runner
                    .run("fail2ban-client", &["reload"], Duration::from_secs(10))
                    .map(|o| o.success())
                    .unwrap_or(false);
                if !reloaded {
                    let _ = runner.run(
                        "systemctl",
                        &["reload-or-restart", "fail2ban"],
                        Duration::from_secs(10),
                    );
                }
            }
        }
    }

    Ok(F2bSyncReport {
        ssh_port,
        tcp_ports_nft,
        udp_ports_nft,
        all_public_ports_f2b,
        guard_changed,
        jail_changed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::MockCommandRunner;
    use tempfile::TempDir;

    fn base_opts(root: &Path) -> F2bSyncOptions {
        F2bSyncOptions {
            root_prefix: root.to_path_buf(),
            no_sys: true,
            force_reload: false,
            print_ports: false,
            backend_override: Some("systemd".to_string()),
            template_dir: None,
            ss_file: None,
        }
    }

    #[test]
    fn ssh_port_priority_nft_socket_sshd_fallback() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let runner = MockCommandRunner::new();
        let opts = base_opts(root);

        // Fallback 22
        assert_eq!(extract_ssh_port(&opts, &runner), 22);

        // sshd_config Port
        fs::create_dir_all(root.join("etc/ssh")).unwrap();
        fs::write(root.join("etc/ssh/sshd_config"), "Port 2200\n").unwrap();
        assert_eq!(extract_ssh_port(&opts, &runner), 2200);

        // ssh.socket override takes precedence over sshd_config (last non-empty wins)
        fs::create_dir_all(root.join("etc/systemd/system/ssh.socket.d")).unwrap();
        fs::write(
            root.join("etc/systemd/system/ssh.socket.d/override.conf"),
            "[Socket]\nListenStream=\nListenStream=0.0.0.0:3300\nListenStream=[::]:3301\n",
        )
        .unwrap();
        assert_eq!(extract_ssh_port(&opts, &runner), 3301);

        // nftables.conf SSH_Access_Port takes highest precedence
        fs::create_dir_all(root.join("etc")).unwrap();
        fs::write(
            root.join("etc/nftables.conf"),
            "tcp dport 50022 accept # SSH_Access_Port\n",
        )
        .unwrap();
        assert_eq!(extract_ssh_port(&opts, &runner), 50022);
    }

    #[test]
    fn full_f2b_sync_lifecycle_matches_bash_parity() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let engine = TemplateEngine::new(None);
        let runner = MockCommandRunner::new();

        // 1. Initial nftables.conf with SSH port 2222
        let mut nft_vars = BTreeMap::new();
        nft_vars.insert("SSH_PORT".to_string(), "2222".to_string());
        nft_vars.insert(
            "NFT_BASE_D".to_string(),
            root.join("etc/nftables.d").to_string_lossy().into_owned(),
        );
        engine
            .render_to_file(
                "templates/security/nftables.conf",
                &root.join("etc/nftables.conf"),
                &nft_vars,
                None,
            )
            .unwrap();

        let opts = base_opts(root);
        let r1 = run_f2b_sync(&opts, &engine, &runner).unwrap();
        assert_eq!(r1.ssh_port, 2222);
        assert_eq!(r1.tcp_ports_nft, "2222");
        assert_eq!(r1.udp_ports_nft, "");
        assert_eq!(r1.all_public_ports_f2b, "2222");
        assert!(r1.guard_changed);
        assert!(r1.jail_changed);

        let jail_path = root.join("etc/fail2ban/jail.d/99-debopty-ports.local");
        let guard_path = root.join("etc/nftables.d/55-debopty-f2b-guard.nft");
        let jail_txt = fs::read_to_string(&jail_path).unwrap();
        let guard_txt = fs::read_to_string(&guard_path).unwrap();
        assert!(jail_txt.contains("port = 2222"));
        assert!(jail_txt.contains("enabled = true"));
        assert!(jail_txt.contains("backend = systemd"));
        assert!(jail_txt.contains("journalmatch = _TRANSPORT=kernel"));
        assert!(guard_txt.contains("tcp dport 2222 update @abuse_v4"));
        assert!(!guard_txt.contains("udp dport"));

        // 2. Add single (41641/udp), list (80,443/tcp/udp), range (11010-11015/tcp/udp), SSH=50022
        nft_vars.insert("SSH_PORT".to_string(), "50022".to_string());
        engine
            .render_to_file(
                "templates/security/nftables.conf",
                &root.join("etc/nftables.conf"),
                &nft_vars,
                None,
            )
            .unwrap();
        fs::create_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
        fs::write(
            root.join("etc/nftables.d/debopti/tailscale.nft"),
            "udp dport 41641 accept\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/nftables.d/debopti/caddy.nft"),
            "tcp dport { 80, 443 } accept\nudp dport { 80, 443 } accept\n",
        )
        .unwrap();
        fs::write(
            root.join("etc/nftables.d/debopti/easytier.nft"),
            "tcp dport 11010-11015 accept\nudp dport 11010-11015 accept\n",
        )
        .unwrap();

        let r2 = run_f2b_sync(&opts, &engine, &runner).unwrap();
        assert_eq!(r2.ssh_port, 50022);
        assert_eq!(r2.all_public_ports_f2b, "80,443,11010:11015,41641,50022");
        assert_eq!(r2.tcp_ports_nft, "{ 80, 443, 11010 - 11015, 50022 }");
        assert_eq!(r2.udp_ports_nft, "{ 80, 443, 11010 - 11015, 41641 }");
        assert!(r2.guard_changed);
        assert!(r2.jail_changed);

        // 3. Idempotent re-run must report changed=false
        let r3 = run_f2b_sync(&opts, &engine, &runner).unwrap();
        assert!(!r3.guard_changed);
        assert!(!r3.jail_changed);

        // 4. Zero open ports removes guard nft and disables public-ports jail
        fs::remove_file(root.join("etc/nftables.conf")).unwrap();
        fs::remove_dir_all(root.join("etc/nftables.d/debopti")).unwrap();
        let r4 = run_f2b_sync(&opts, &engine, &runner).unwrap();
        assert!(!guard_path.exists());
        assert!(r4.guard_changed);
        assert!(r4.jail_changed);
        let jail_zero = fs::read_to_string(&jail_path).unwrap();
        assert!(jail_zero.contains("enabled = false"));
    }

    #[test]
    fn ss_fallback_excludes_loopback_and_link_local() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let engine = TemplateEngine::new(None);
        let runner = MockCommandRunner::new();

        let ss_path = root.join("ss.txt");
        fs::write(
            &ss_path,
            "\
tcp LISTEN 0 128 127.0.0.1:8080 0.0.0.0:*
tcp LISTEN 0 128 [::1]:9090 [::]:*
tcp LISTEN 0 128 169.254.1.1:1234 0.0.0.0:*
tcp LISTEN 0 128 [fe80::1%eth0]:5678 [::]:*
tcp LISTEN 0 128 0.0.0.0:443 0.0.0.0:*
udp UNCONN 0 0 [::]:41641 [::]:*
",
        )
        .unwrap();

        let mut opts = base_opts(root);
        opts.print_ports = true;
        opts.ss_file = Some(ss_path);

        let report = run_f2b_sync(&opts, &engine, &runner).unwrap();
        assert_eq!(report.tcp_ports_nft, "443");
        assert_eq!(report.udp_ports_nft, "41641");
        assert_eq!(report.all_public_ports_f2b, "443,41641");
        assert!(!report.guard_changed);
        assert!(!report.jail_changed);
    }

    #[test]
    fn public_guard_backend_respects_root_prefix_without_host_leak() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let runner = MockCommandRunner::new();
        runner.on_exact(
            "python3",
            &["-c", "import systemd.journal"],
            crate::sys::CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );

        let mut opts = base_opts(root);
        opts.backend_override = None;

        // Without <root>/run/systemd/system, must NOT leak host /run/systemd/system
        assert_eq!(
            detect_public_guard_backend(&opts, &runner),
            "logpath = /var/log/kern.log"
        );

        // Once <root>/run/systemd/system exists, selects systemd backend
        fs::create_dir_all(root.join("run/systemd/system")).unwrap();
        assert_eq!(
            detect_public_guard_backend(&opts, &runner),
            "backend = systemd\njournalmatch = _TRANSPORT=kernel"
        );
    }
}
