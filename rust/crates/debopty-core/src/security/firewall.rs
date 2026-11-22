use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use tempfile::NamedTempFile;

use crate::error::{CoreError, Result};
use crate::security::f2b_sync;
use crate::security::fail2ban;
use crate::security::{ensure_trailing_newline, write_file_atomic};
use crate::sys::CommandRunner;

pub use crate::security::SecurityContext;

fn flush_ruleset_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*flush\s+ruleset\s*;?\s*$").expect("valid flush ruleset re"))
}

/// Replace any legacy global `flush ruleset` line with table-scoped `table inet filter {}` +
/// `flush table inet filter` so container/Docker/Podman/Tailscale/Fail2ban tables are never wiped.
pub(crate) fn scope_global_flush_ruleset(content: &str) -> String {
    let mut changed = false;
    let mut out = Vec::new();
    for line in content.lines() {
        let code = line.split('#').next().unwrap_or(line);
        if flush_ruleset_re().is_match(code) {
            out.push("table inet filter {}".to_string());
            out.push("flush table inet filter".to_string());
            changed = true;
        } else {
            out.push(line.to_string());
        }
    }
    if !changed {
        return ensure_trailing_newline(content.to_string());
    }
    let mut rendered = out.join("\n");
    rendered.push('\n');
    rendered
}

/// Ensure `/etc/nftables.conf` does not contain a destructive global `flush ruleset`.
fn ensure_scoped_nft_main_conf(conf: &Path) -> Result<()> {
    if !conf.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(conf)?;
    let scoped = scope_global_flush_ruleset(&raw);
    if scoped != raw {
        write_file_atomic(conf, scoped.as_bytes(), None)?;
    }
    Ok(())
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

fn chain_input_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*chain\s+input\s*\{").expect("valid chain input regex"))
}

/// Validate a firewall rule comment/identifier and return its sanitized `.nft` filename stem.
///
/// Rejects path traversal (`..`, `/`, `\`), control characters, quotes, semicolons, and any
/// characters outside `[A-Za-z0-9._ -]` (sanitized stem must match `^[A-Za-z0-9._-]{1,64}$`).
pub fn validate_fw_comment(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(CoreError::ValidationFailed(format!(
            "firewall comment must be 1..=64 chars: '{raw}'"
        )));
    }
    if !trimmed
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b' '))
    {
        return Err(CoreError::ValidationFailed(format!(
            "firewall comment contains forbidden characters: '{raw}'"
        )));
    }
    let sanitized = trimmed.replace(' ', "_");
    if sanitized.is_empty()
        || sanitized.len() > 64
        || sanitized == "."
        || sanitized == ".."
        || sanitized.contains("..")
        || sanitized.starts_with('.')
        || sanitized.starts_with('-')
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid firewall rule identifier: '{raw}'"
        )));
    }
    Ok(sanitized)
}

/// Validate a firewall protocol specification (`tcp`, `udp`, or `tcp/udp`).
pub fn validate_fw_proto(raw: &str) -> Result<Vec<&'static str>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(vec!["tcp"]);
    }
    let mut protos: Vec<&'static str> = Vec::new();
    for part in trimmed.split('/') {
        let tok = part.trim();
        let canonical = match tok {
            "tcp" => "tcp",
            "udp" => "udp",
            _ => {
                return Err(CoreError::ValidationFailed(format!(
                    "invalid protocol specification (allowed: tcp, udp, tcp/udp): '{raw}'"
                )));
            }
        };
        if !protos.contains(&canonical) {
            protos.push(canonical);
        }
    }
    if protos.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "invalid protocol specification: '{raw}'"
        )));
    }
    Ok(protos)
}

/// Validate a port, port list (`80,443`), port range (`11010-11015` / `11010:11015`),
/// or braced set (`{ 80, 443 }`) strictly within `1..=65535`.
pub fn validate_fw_port(raw: &str) -> Result<()> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(CoreError::ValidationFailed(
            "firewall port specification cannot be empty".to_string(),
        ));
    }

    let inner = if trimmed.starts_with('{') || trimmed.ends_with('}') {
        if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
            return Err(CoreError::ValidationFailed(format!(
                "unbalanced braces in port specification: '{raw}'"
            )));
        }
        let body = trimmed[1..trimmed.len() - 1].trim();
        if body.contains('{') || body.contains('}') {
            return Err(CoreError::ValidationFailed(format!(
                "nested braces in port specification: '{raw}'"
            )));
        }
        body
    } else {
        trimmed
    };

    if inner.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "empty port specification: '{raw}'"
        )));
    }

    for item in inner.split(',') {
        let part = item.trim();
        if part.is_empty() {
            return Err(CoreError::ValidationFailed(format!(
                "empty port entry in specification: '{raw}'"
            )));
        }

        if let Some((a_raw, b_raw)) = part.split_once('-').or_else(|| part.split_once(':')) {
            let a_str = a_raw.trim();
            let b_str = b_raw.trim();
            if a_str.is_empty()
                || b_str.is_empty()
                || !a_str.bytes().all(|b| b.is_ascii_digit())
                || !b_str.bytes().all(|b| b.is_ascii_digit())
            {
                return Err(CoreError::ValidationFailed(format!(
                    "invalid port range '{part}' in '{raw}'"
                )));
            }
            let start = a_str.parse::<u16>().map_err(|_| {
                CoreError::ValidationFailed(format!("port out of range in '{raw}'"))
            })?;
            let end = b_str.parse::<u16>().map_err(|_| {
                CoreError::ValidationFailed(format!("port out of range in '{raw}'"))
            })?;
            if start == 0 || start > end {
                return Err(CoreError::ValidationFailed(format!(
                    "invalid port range {start}-{end} in '{raw}'"
                )));
            }
        } else {
            if !part.bytes().all(|b| b.is_ascii_digit()) {
                return Err(CoreError::ValidationFailed(format!(
                    "invalid port token '{part}' in '{raw}'"
                )));
            }
            let p = part.parse::<u16>().map_err(|_| {
                CoreError::ValidationFailed(format!("port out of range in '{raw}'"))
            })?;
            if p == 0 {
                return Err(CoreError::ValidationFailed(format!(
                    "port 0 is not allowed in '{raw}'"
                )));
            }
        }
    }

    Ok(())
}

fn default_comment_for_port(port: &str) -> String {
    let mut out = String::from("port_");
    let mut last_us = true;
    for ch in port.trim().chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '.' {
            out.push(ch);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    let trimmed = out.trim_end_matches('_');
    let capped: String = trimmed.chars().take(64).collect();
    if capped.is_empty() || capped == "port" {
        "port_rule".to_string()
    } else {
        capped
    }
}

/// Format port / port-range / port-list expression for `nftables` syntax.
///
/// Mirrors `scripts/security.sh::add_fw_rule`:
/// - Converts `-` and `:` to `" - "`.
/// - Wraps in `"{ ... }"` when containing whitespace or `,` (unless already starting with `{`).
pub fn format_nft_port(raw: &str) -> String {
    let mut nft_port = raw.replace(['-', ':'], " - ");
    if (nft_port.contains(' ') || nft_port.contains(',')) && !nft_port.starts_with('{') {
        nft_port = format!("{{ {nft_port} }}");
    }
    nft_port
}

/// Migrate legacy `/etc/nftables/debopti.d/*.nft` rules into `ctx.effective_nft_conf_dir()`.
///
/// Returns `true` if a legacy directory existed and was migrated.
pub fn migrate_nft_paths(ctx: &SecurityContext) -> Result<bool> {
    let old_root = ctx.path("/etc/nftables");
    let old_dir = old_root.join("debopti.d");
    if !old_dir.is_dir() {
        return Ok(false);
    }

    let nft_conf_dir = ctx.effective_nft_conf_dir();
    let nft_main_conf = ctx.effective_nft_main_conf();

    fs::create_dir_all(&nft_conf_dir)?;

    for entry in fs::read_dir(&old_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path.extension().is_some_and(|e| e == "nft")
            && let Some(file_name) = path.file_name()
        {
            let dest = nft_conf_dir.join(file_name);
            if !dest.exists() {
                fs::copy(&path, &dest)?;
            }
        }
    }

    if nft_main_conf.is_file() {
        let content = fs::read_to_string(&nft_main_conf)?;
        let legacy_include_abs = "include \"/etc/nftables/debopti.d/*.nft\"";
        let legacy_include_prefixed = format!("include \"{}/*.nft\"", old_dir.display());
        let filtered: Vec<&str> = content
            .lines()
            .filter(|line| {
                !line.contains(legacy_include_abs) && !line.contains(&legacy_include_prefixed)
            })
            .collect();
        let mut rendered = filtered.join("\n");
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        write_file_atomic(&nft_main_conf, rendered.as_bytes(), None)?;
    }

    fs::remove_dir_all(&old_dir)?;
    // Remove legacy parent directory only if it is now empty (never delete unrelated files).
    let _ = fs::remove_dir(&old_root);

    for obsolete in ["SSH_Hardened_Port.nft", "SSH_Listen_Port.nft"] {
        let p = nft_conf_dir.join(obsolete);
        if p.exists() {
            let _ = fs::remove_file(p);
        }
    }

    Ok(true)
}

/// Update the `SSH_Access_Port` rule in `nftables.conf` to allow one or more SSH ports,
/// validate with `nft -c -f` before loading with `nft -f`, and sync Fail2ban.
pub fn update_ssh_firewall_ports(
    ctx: &SecurityContext,
    ports: &[u16],
    runner: &dyn CommandRunner,
) -> Result<()> {
    let mut port_set = BTreeSet::new();
    for &p in ports {
        if p == 0 {
            return Err(CoreError::ValidationFailed(
                "SSH firewall port must be in 1..=65535".to_string(),
            ));
        }
        port_set.insert(p);
    }
    if port_set.is_empty() {
        return Err(CoreError::ValidationFailed(
            "at least one SSH firewall port is required".to_string(),
        ));
    }

    let conf = ctx.effective_nft_main_conf();
    if !conf.is_file() {
        let primary = *port_set.iter().next().expect("non-empty port set");
        fail2ban::setup_security_with_port(ctx, Some(primary), runner)?;
        if port_set.len() == 1 {
            return Ok(());
        }
    }

    let content = fs::read_to_string(&conf)?;
    let port_expr = if port_set.len() == 1 {
        port_set.iter().next().expect("single port").to_string()
    } else {
        let joined = port_set
            .iter()
            .map(u16::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{{ {joined} }}")
    };
    let new_rule = format!(r#"tcp dport {port_expr} accept comment "SSH_Access_Port""#);

    let updated = if content.contains(r#""SSH_Access_Port""#) {
        if !ssh_access_rule_re().is_match(&content) {
            return Err(CoreError::Security(format!(
                "existing SSH_Access_Port rule in {} could not be parsed safely",
                conf.display()
            )));
        }
        ssh_access_rule_re()
            .replace_all(&content, new_rule.as_str())
            .into_owned()
    } else {
        let mut out_lines: Vec<String> = Vec::new();
        let mut inserted = false;
        for line in content.lines() {
            out_lines.push(line.to_string());
            if !inserted && chain_input_re().is_match(line) {
                out_lines.push(format!("        {new_rule}"));
                inserted = true;
            }
        }
        if !inserted {
            return Err(CoreError::Security(format!(
                "could not locate 'chain input {{' in {} to insert SSH_Access_Port rule",
                conf.display()
            )));
        }
        let mut joined = out_lines.join("\n");
        joined.push('\n');
        joined
    };

    let rendered = scope_global_flush_ruleset(&updated);

    if !ctx.no_sys {
        // Stage and dry-run validate with `nft -c -f` BEFORE mutating `/etc/nftables.conf`.
        let parent = conf
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let mut staged = NamedTempFile::new_in(parent)?;
        staged.write_all(rendered.as_bytes())?;
        staged.flush()?;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o644))?;

        let staged_str = staged.path().to_string_lossy().into_owned();
        let check = runner.run("nft", &["-c", "-f", &staged_str], Duration::from_secs(15))?;
        if !check.success() {
            return Err(CoreError::Security(format!(
                "nftables dry-run validation failed for {}: {}",
                conf.display(),
                check.stderr.trim()
            )));
        }

        write_file_atomic(&conf, rendered.as_bytes(), None)?;
        let conf_str = conf.to_string_lossy();
        let out = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15))?;
        if !out.success() {
            let _ = write_file_atomic(&conf, content.as_bytes(), None);
            return Err(CoreError::Security(format!(
                "nftables reload failed for {}: {}",
                conf.display(),
                out.stderr.trim()
            )));
        }
    } else {
        write_file_atomic(&conf, rendered.as_bytes(), None)?;
    }

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(())
}

/// Update the `SSH_Access_Port` rule in `nftables.conf`, reload `nftables`, and sync Fail2ban.
pub fn update_ssh_firewall(
    ctx: &SecurityContext,
    port: u16,
    runner: &dyn CommandRunner,
) -> Result<()> {
    update_ssh_firewall_ports(ctx, &[port], runner)
}

/// Render and load a modular nftables rule under `ctx.effective_nft_conf_dir()`.
///
/// Supports single ports, ranges (`11010-11015` / `11010:11015`), lists (`80,443`),
/// and compound protocols (`tcp/udp`). Validates all inputs strictly, runs `nft -c -f`
/// dry-run before `nft -f`, and deletes the rule file on failure.
pub fn add_fw_rule(
    ctx: &SecurityContext,
    port: &str,
    proto: &str,
    comment: &str,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    let port_trimmed = port.trim();
    validate_fw_port(port_trimmed)?;
    let protos = validate_fw_proto(proto)?;
    let eff_proto = protos.join("/");

    let eff_comment = if comment.trim().is_empty() {
        default_comment_for_port(port_trimmed)
    } else {
        comment.trim().to_string()
    };
    let sanitized = validate_fw_comment(&eff_comment)?;

    let nft_base_d = ctx.effective_nft_base_d();
    let nft_conf_dir = ctx.effective_nft_conf_dir();
    let nft_main_conf = ctx.effective_nft_main_conf();

    if !nft_conf_dir.is_dir() {
        if ctx.no_sys {
            fs::create_dir_all(&nft_conf_dir)?;
        } else {
            fail2ban::setup_security(ctx, runner)?;
            fs::create_dir_all(&nft_conf_dir)?;
        }
    }

    let nft_port = format_nft_port(port_trimmed);
    let rule_lines: Vec<String> = protos
        .iter()
        .map(|p| format!("        {p} dport {nft_port} accept comment \"{eff_comment}\""))
        .collect();

    let rule_file = nft_conf_dir.join(format!("{sanitized}.nft"));
    if rule_file.parent() != Some(nft_conf_dir.as_path()) {
        return Err(CoreError::ValidationFailed(format!(
            "firewall rule path escapes nftables directory: '{comment}'"
        )));
    }

    let engine = ctx.template_engine();
    let mut vars = BTreeMap::new();
    vars.insert("RULES".to_string(), rule_lines.join("\n"));
    let rendered = ensure_trailing_newline(
        engine.render_to_string("templates/security/rule_template.nft", &vars)?,
    );

    let prev_rule = fs::read(&rule_file).ok();

    if !ctx.no_sys {
        // Stage into a sibling temp file and validate with `nft -c -f` BEFORE touching `rule_file`,
        // preserving any pre-existing valid rule on syntax failure.
        let mut staged = NamedTempFile::new_in(&nft_conf_dir)?;
        staged.write_all(rendered.as_bytes())?;
        staged.flush()?;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o644))?;

        let staged_str = staged.path().to_string_lossy().into_owned();
        let check_ok = runner
            .run("nft", &["-c", "-f", &staged_str], Duration::from_secs(15))
            .map(|o| o.success())
            .unwrap_or(false);
        if !check_ok {
            return Err(CoreError::Security(format!(
                "nftables dry-run check failed for {port_trimmed}/{eff_proto} ({eff_comment})"
            )));
        }
    }

    write_file_atomic(&rule_file, rendered.as_bytes(), Some(0o644))?;

    let include_base_line = format!("include \"{}/*.nft\"", nft_base_d.display());
    if nft_main_conf.is_file() {
        let main_content = fs::read_to_string(&nft_main_conf)?;
        let mut next = scope_global_flush_ruleset(&main_content);
        if !next.contains(&include_base_line) {
            next = ensure_trailing_newline(next);
            next.push_str(&include_base_line);
            next.push('\n');
        }
        if next != main_content {
            write_file_atomic(&nft_main_conf, next.as_bytes(), None)?;
        }
    }

    let mod_entry = nft_base_d.join("50-debopti.nft");
    if !mod_entry.is_file() {
        let mod_content = format!("include \"{}/*.nft\"\n", nft_conf_dir.display());
        write_file_atomic(&mod_entry, mod_content.as_bytes(), Some(0o644))?;
    }

    if !ctx.no_sys {
        // Reload table-scoped `nftables.conf` when present so repeated `add_fw_rule` calls
        // never duplicate rules in the live kernel chain (and never flush external tables).
        let load_ok = if nft_main_conf.is_file() {
            let conf_str = nft_main_conf.to_string_lossy();
            let check_main = runner
                .run("nft", &["-c", "-f", &conf_str], Duration::from_secs(15))
                .map(|o| o.success())
                .unwrap_or(false);
            check_main
                && runner
                    .run("nft", &["-f", &conf_str], Duration::from_secs(15))
                    .map(|o| o.success())
                    .unwrap_or(false)
        } else {
            let rule_str = rule_file.to_string_lossy();
            runner
                .run("nft", &["-f", &rule_str], Duration::from_secs(15))
                .map(|o| o.success())
                .unwrap_or(false)
        };
        if !load_ok {
            if let Some(bytes) = prev_rule {
                let _ = write_file_atomic(&rule_file, &bytes, Some(0o644));
            } else {
                let _ = fs::remove_file(&rule_file);
            }
            return Err(CoreError::Security(format!(
                "nftables rule load failed for {port_trimmed}/{eff_proto} ({eff_comment})"
            )));
        }
    }

    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(rule_file)
}

/// Remove a modular nftables rule by comment name, reload main config, and sync Fail2ban.
///
/// Returns `true` if the rule file existed and was removed.
pub fn remove_fw_rule(
    ctx: &SecurityContext,
    comment: &str,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    let sanitized = validate_fw_comment(comment)?;
    let nft_conf_dir = ctx.effective_nft_conf_dir();
    let rule_file = nft_conf_dir.join(format!("{sanitized}.nft"));
    if rule_file.parent() != Some(nft_conf_dir.as_path()) {
        return Err(CoreError::ValidationFailed(format!(
            "firewall rule path escapes nftables directory: '{comment}'"
        )));
    }
    if !rule_file.is_file() {
        return Ok(false);
    }

    let prev_rule = fs::read(&rule_file).ok();
    fs::remove_file(&rule_file)?;

    let nft_main_conf = ctx.effective_nft_main_conf();
    if !ctx.no_sys && nft_main_conf.is_file() {
        ensure_scoped_nft_main_conf(&nft_main_conf)?;
        let conf_str = nft_main_conf.to_string_lossy();
        let check_ok = runner
            .run("nft", &["-c", "-f", &conf_str], Duration::from_secs(15))
            .map(|o| o.success())
            .unwrap_or(false);
        if check_ok {
            let load_ok = runner
                .run("nft", &["-f", &conf_str], Duration::from_secs(15))
                .map(|o| o.success())
                .unwrap_or(false);
            if !load_ok && let Some(bytes) = prev_rule {
                let _ = write_file_atomic(&rule_file, &bytes, Some(0o644));
                return Err(CoreError::Security(format!(
                    "nftables reload failed after removing rule '{comment}'"
                )));
            }
        } else if let Some(bytes) = prev_rule {
            let _ = write_file_atomic(&rule_file, &bytes, Some(0o644));
            return Err(CoreError::Security(format!(
                "nftables dry-run validation failed after removing rule '{comment}'"
            )));
        }
    }

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(true)
}

/// Idempotently patch legacy `nftables.conf` with IPv6 NDP + PMTU allow rules after `icmpv6 type echo-request`.
///
/// Returns `true` if the file was modified.
pub fn patch_nftables_ndp(ctx: &SecurityContext, runner: &dyn CommandRunner) -> Result<bool> {
    let conf = ctx.effective_nft_main_conf();
    if !conf.is_file() {
        return Ok(false);
    }

    let content = fs::read_to_string(&conf)?;
    if !content.contains("SSH_Access_Port") || content.contains("nd-router-solicit") {
        return Ok(false);
    }

    let mut out_lines: Vec<String> = Vec::new();
    let mut patched = false;
    for line in content.lines() {
        out_lines.push(line.to_string());
        if line.contains("icmpv6 type echo-request") {
            out_lines.push(
                "        # 核心：放行 IPv6 邻居发现协议 (NDP) 与 PMTU (packet-too-big)，防止 IPv6 断网与隧道分片异常".to_string(),
            );
            out_lines.push(
                "        ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert, packet-too-big } accept"
                    .to_string(),
            );
            patched = true;
        }
    }

    if !patched {
        return Ok(false);
    }

    let mut rendered = out_lines.join("\n");
    rendered.push('\n');
    write_file_atomic(&conf, rendered.as_bytes(), None)?;

    if !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &["reload", "nftables"],
            Duration::from_secs(15),
        );
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};
    use tempfile::TempDir;

    fn ok_out() -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_out(msg: &str) -> CmdOutput {
        CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: msg.to_string(),
            timed_out: false,
        }
    }

    #[test]
    fn format_nft_port_matches_bash_rules() {
        assert_eq!(format_nft_port("443"), "443");
        assert_eq!(format_nft_port("80,443"), "{ 80,443 }");
        assert_eq!(format_nft_port("11010-11015"), "{ 11010 - 11015 }");
        assert_eq!(format_nft_port("11010:11015"), "{ 11010 - 11015 }");
        assert_eq!(format_nft_port("{ 80, 443 }"), "{ 80, 443 }");
    }

    #[test]
    fn firewall_crud_update_ssh_and_f2b_sync() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = true;
        ctx.f2b_backend_override = Some("systemd".to_string());
        let runner = MockCommandRunner::new();

        // Render initial nftables.conf with SSH port 2222
        let engine = ctx.template_engine();
        let mut vars = BTreeMap::new();
        vars.insert("SSH_PORT".to_string(), "2222".to_string());
        vars.insert(
            "NFT_BASE_D".to_string(),
            ctx.effective_nft_base_d().to_string_lossy().into_owned(),
        );
        engine
            .render_to_file(
                "templates/security/nftables.conf",
                &ctx.effective_nft_main_conf(),
                &vars,
                Some(0o644),
            )
            .unwrap();

        update_ssh_firewall(&ctx, 50022, &runner).unwrap();
        let conf_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(conf_txt.contains(r#"tcp dport 50022 accept comment "SSH_Access_Port""#));
        assert!(!conf_txt.contains("2222"));

        let rule1 = add_fw_rule(&ctx, "41641", "udp", "Tailscale P2P", &runner).unwrap();
        assert_eq!(rule1.file_name().unwrap(), "Tailscale_P2P.nft");
        add_fw_rule(&ctx, "80,443", "tcp/udp", "Caddy_Web", &runner).unwrap();
        add_fw_rule(&ctx, "11010-11015", "tcp/udp", "Easytier_P2P", &runner).unwrap();

        let caddy_txt =
            fs::read_to_string(ctx.effective_nft_conf_dir().join("Caddy_Web.nft")).unwrap();
        assert!(caddy_txt.contains(r#"tcp dport { 80,443 } accept comment "Caddy_Web""#));
        assert!(caddy_txt.contains(r#"udp dport { 80,443 } accept comment "Caddy_Web""#));

        let jail_ports =
            fs::read_to_string(ctx.path("/etc/fail2ban/jail.d/99-debopty-ports.local")).unwrap();
        assert!(jail_ports.contains("port = 80,443,11010:11015,41641,50022"));
        assert!(ctx.effective_nft_base_d().join("50-debopti.nft").is_file());

        // Remove rules and verify f2b sync cleans them
        assert!(remove_fw_rule(&ctx, "Easytier_P2P", &runner).unwrap());
        assert!(remove_fw_rule(&ctx, "Tailscale P2P", &runner).unwrap());
        assert!(!remove_fw_rule(&ctx, "Nonexistent_Rule", &runner).unwrap());

        let jail_after =
            fs::read_to_string(ctx.path("/etc/fail2ban/jail.d/99-debopty-ports.local")).unwrap();
        assert!(jail_after.contains("port = 80,443,50022"));
        assert!(!jail_after.contains("41641"));
        assert!(!jail_after.contains("11010:11015"));
    }

    #[test]
    fn add_fw_rule_deletes_invalid_rule_on_nft_failure() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        fs::create_dir_all(ctx.effective_nft_conf_dir()).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("nft", err_out("syntax error"));

        let err = add_fw_rule(&ctx, "8080", "tcp", "Broken_Rule", &runner).unwrap_err();
        assert!(matches!(err, CoreError::Security(_)));
        assert!(
            !ctx.effective_nft_conf_dir()
                .join("Broken_Rule.nft")
                .exists()
        );

        // Verify dry-run `nft -c -f` ran first and live `nft -f` was NEVER invoked
        let calls = runner.invocations();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "nft");
        assert_eq!(calls[0].1[0], "-c");
        assert_eq!(calls[0].1[1], "-f");
    }

    #[test]
    fn fw_rule_rejects_path_traversal_and_nft_injection() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        fs::create_dir_all(ctx.effective_nft_conf_dir()).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("nft", ok_out());

        // Path traversal & quote/newline injection in comment
        for bad_comment in [
            "../evil",
            "../../etc/nftables",
            "a/b",
            "..",
            ".hidden",
            "-flag",
            "x\"\nflush ruleset\n#",
            "a\"; flush ruleset",
        ] {
            let err = add_fw_rule(&ctx, "443", "tcp", bad_comment, &runner).unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for comment={bad_comment:?}, got {err:?}"
            );
            let rm_err = remove_fw_rule(&ctx, bad_comment, &runner).unwrap_err();
            assert!(
                matches!(rm_err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed on remove for comment={bad_comment:?}, got {rm_err:?}"
            );
        }

        // Protocol injection
        for bad_proto in [
            "tcp;drop",
            "tcp dport 1 accept; flush ruleset; tcp",
            "icmp",
            "tcp/",
            "/udp",
        ] {
            let err = add_fw_rule(&ctx, "443", bad_proto, "Valid_Name", &runner).unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for proto={bad_proto:?}, got {err:?}"
            );
        }

        // Port injection & out-of-range
        for bad_port in [
            "", "0", "65536", "bad", "80;flush", "100-50", "80,", "{80", "{{80}}", "80 443",
        ] {
            let err = add_fw_rule(&ctx, bad_port, "tcp", "Valid_Name", &runner).unwrap_err();
            assert!(
                matches!(err, CoreError::ValidationFailed(_)),
                "expected ValidationFailed for port={bad_port:?}, got {err:?}"
            );
        }

        // Zero nft commands executed and zero files written
        assert!(runner.invocations().is_empty());
        assert_eq!(
            fs::read_dir(ctx.effective_nft_conf_dir()).unwrap().count(),
            0
        );

        // Valid rule runs `nft -c -f` strictly before `nft -f`
        let ok_rule = add_fw_rule(&ctx, "80,443", "tcp/udp", "Safe_Web", &runner).unwrap();
        assert!(ok_rule.is_file());
        let nft_calls: Vec<_> = runner
            .invocations()
            .into_iter()
            .filter(|(p, _)| p == "nft")
            .collect();
        assert!(nft_calls.len() >= 2);
        assert_eq!(nft_calls[0].1[0], "-c");
        assert_eq!(nft_calls[0].1[1], "-f");
        assert_eq!(nft_calls[1].1[0], "-f");
    }

    #[test]
    fn update_ssh_firewall_handles_braced_ports_preserves_0644_and_fails_on_unmatched_chain() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = true;
        ctx.f2b_backend_override = Some("systemd".to_string());
        let runner = MockCommandRunner::new();

        // 1. Braced multi-port rule with tabs is matched and updated, mode 0644 preserved
        let conf = ctx.effective_nft_main_conf();
        fs::create_dir_all(conf.parent().unwrap()).unwrap();
        fs::write(
            &conf,
            "table inet filter {\n    chain input {\n\ttcp\tdport\t{ 22, 2222 }\taccept\tcomment\t\"SSH_Access_Port\"\n    }\n}\n",
        )
        .unwrap();
        fs::set_permissions(&conf, fs::Permissions::from_mode(0o644)).unwrap();

        update_ssh_firewall(&ctx, 50022, &runner).unwrap();
        let updated = fs::read_to_string(&conf).unwrap();
        assert!(updated.contains(r#"tcp dport 50022 accept comment "SSH_Access_Port""#));
        assert!(!updated.contains("2222"));
        assert_eq!(
            fs::metadata(&conf).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // 2. Malformed nftables.conf lacking `chain input {` fails closed (never silent no-op)
        fs::write(&conf, "table inet filter {\n    chain output {\n    }\n}\n").unwrap();
        let err = update_ssh_firewall(&ctx, 50022, &runner).unwrap_err();
        assert!(matches!(err, CoreError::Security(_)));
    }

    #[test]
    fn update_ssh_firewall_bootstraps_missing_conf_with_requested_port() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = true;
        ctx.f2b_backend_override = Some("systemd".to_string());
        let runner = MockCommandRunner::new();

        update_ssh_firewall(&ctx, 51234, &runner).unwrap();
        let conf_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(conf_txt.contains(r#"tcp dport 51234 accept comment "SSH_Access_Port""#));
    }

    #[test]
    fn migrate_nft_paths_preserves_unrelated_files_in_etc_nftables() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());

        let old_root = ctx.path("/etc/nftables");
        let old_dir = old_root.join("debopti.d");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("app.nft"), "tcp dport 8080 accept\n").unwrap();
        let unrelated = old_root.join("custom-user.nft");
        fs::write(&unrelated, "# keep me\n").unwrap();

        assert!(migrate_nft_paths(&ctx).unwrap());
        assert!(!old_dir.exists());
        assert!(unrelated.is_file());
        assert_eq!(fs::read_to_string(&unrelated).unwrap(), "# keep me\n");
        assert!(ctx.effective_nft_conf_dir().join("app.nft").is_file());
    }

    #[test]
    fn migrate_nft_paths_moves_rules_strips_include_and_cleans_legacy_ssh() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());

        let old_dir = ctx.path("/etc/nftables/debopti.d");
        fs::create_dir_all(&old_dir).unwrap();
        fs::write(old_dir.join("app.nft"), "tcp dport 8080 accept\n").unwrap();
        fs::write(old_dir.join("SSH_Hardened_Port.nft"), "old\n").unwrap();
        fs::write(old_dir.join("SSH_Listen_Port.nft"), "old\n").unwrap();

        fs::create_dir_all(ctx.effective_nft_main_conf().parent().unwrap()).unwrap();
        fs::write(
            ctx.effective_nft_main_conf(),
            "flush ruleset\ninclude \"/etc/nftables/debopti.d/*.nft\"\n",
        )
        .unwrap();

        assert!(migrate_nft_paths(&ctx).unwrap());
        assert!(!ctx.path("/etc/nftables").exists());
        assert!(ctx.effective_nft_conf_dir().join("app.nft").is_file());
        assert!(
            !ctx.effective_nft_conf_dir()
                .join("SSH_Hardened_Port.nft")
                .exists()
        );
        assert!(
            !ctx.effective_nft_conf_dir()
                .join("SSH_Listen_Port.nft")
                .exists()
        );

        let main_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(!main_txt.contains("/etc/nftables/debopti.d"));
        assert!(main_txt.contains("flush ruleset"));

        // Second migration is idempotent no-op
        assert!(!migrate_nft_paths(&ctx).unwrap());
    }

    #[test]
    fn patch_nftables_ndp_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = false;
        let runner = MockCommandRunner::new();
        runner.on_prog("systemctl", ok_out());

        let legacy_conf = "\
table inet filter {
    chain input {
        ip6 nexthdr icmpv6 icmpv6 type echo-request limit rate 5/second accept
        tcp dport 22 accept comment \"SSH_Access_Port\"
    }
}
";
        fs::create_dir_all(ctx.effective_nft_main_conf().parent().unwrap()).unwrap();
        fs::write(ctx.effective_nft_main_conf(), legacy_conf).unwrap();

        assert!(patch_nftables_ndp(&ctx, &runner).unwrap());
        let patched = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(patched.contains("nd-router-solicit"));
        assert!(patched.contains("nd-neighbor-advert"));
        assert_eq!(
            runner.invocations(),
            vec![(
                "systemctl".to_string(),
                vec!["reload".to_string(), "nftables".to_string()]
            )]
        );

        // Second call is an idempotent no-op
        assert!(!patch_nftables_ndp(&ctx, &runner).unwrap());
    }
}
