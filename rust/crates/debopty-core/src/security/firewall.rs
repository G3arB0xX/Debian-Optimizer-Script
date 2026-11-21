use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::error::{CoreError, Result};
use crate::security::f2b_sync;
use crate::security::fail2ban;
use crate::security::{ensure_trailing_newline, write_file_atomic};
use crate::sys::CommandRunner;

pub use crate::security::SecurityContext;

fn ssh_access_rule_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"tcp dport [0-9, \-]* accept comment "SSH_Access_Port""#)
            .expect("valid ssh access rule regex")
    })
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

    if let Ok(entries) = fs::read_dir(&old_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == "nft")
                && let Some(file_name) = path.file_name()
            {
                let dest = nft_conf_dir.join(file_name);
                if !dest.exists() {
                    let _ = fs::copy(&path, &dest);
                }
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

    fs::remove_dir_all(&old_root)?;

    for obsolete in ["SSH_Hardened_Port.nft", "SSH_Listen_Port.nft"] {
        let p = nft_conf_dir.join(obsolete);
        if p.exists() {
            let _ = fs::remove_file(p);
        }
    }

    Ok(true)
}

/// Update the `SSH_Access_Port` rule in `nftables.conf`, reload `nftables`, and sync Fail2ban.
pub fn update_ssh_firewall(
    ctx: &SecurityContext,
    port: u16,
    runner: &dyn CommandRunner,
) -> Result<()> {
    let conf = ctx.effective_nft_main_conf();
    if !conf.is_file() {
        fail2ban::setup_security(ctx, runner)?;
        return Ok(());
    }

    let content = fs::read_to_string(&conf)?;
    let new_rule = format!(r#"tcp dport {port} accept comment "SSH_Access_Port""#);

    let updated = if content.contains(r#"comment "SSH_Access_Port""#) {
        ssh_access_rule_re()
            .replace_all(&content, new_rule.as_str())
            .into_owned()
    } else {
        let mut out_lines: Vec<String> = Vec::new();
        let mut inserted = false;
        for line in content.lines() {
            out_lines.push(line.to_string());
            if !inserted && line.contains("chain input {") {
                out_lines.push(format!("        {new_rule}"));
                inserted = true;
            }
        }
        let mut joined = out_lines.join("\n");
        joined.push('\n');
        joined
    };

    let rendered = ensure_trailing_newline(updated);
    write_file_atomic(&conf, rendered.as_bytes(), None)?;

    if !ctx.no_sys {
        let conf_str = conf.to_string_lossy();
        let out = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15))?;
        if !out.success() {
            return Err(CoreError::Security(format!(
                "nftables reload failed for {}: {}",
                conf.display(),
                out.stderr.trim()
            )));
        }
    }

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(())
}

/// Render and load a modular nftables rule under `ctx.effective_nft_conf_dir()`.
///
/// Supports single ports, ranges (`11010-11015` / `11010:11015`), lists (`80,443`),
/// and compound protocols (`tcp/udp`). Deletes the rule file and returns `Err` on `nft -f` failure.
pub fn add_fw_rule(
    ctx: &SecurityContext,
    port: &str,
    proto: &str,
    comment: &str,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    let eff_proto = if proto.trim().is_empty() {
        "tcp"
    } else {
        proto.trim()
    };
    let eff_comment = if comment.trim().is_empty() {
        format!("port_{port}")
    } else {
        comment.trim().to_string()
    };
    let sanitized = eff_comment.replace(' ', "_");

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

    let nft_port = format_nft_port(port);
    let rule_lines: Vec<String> = eff_proto
        .split('/')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| format!("        {p} dport {nft_port} accept comment \"{eff_comment}\""))
        .collect();
    if rule_lines.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "invalid protocol specification: {proto}"
        )));
    }

    let rule_file = nft_conf_dir.join(format!("{sanitized}.nft"));
    let engine = ctx.template_engine();
    let mut vars = BTreeMap::new();
    vars.insert("RULES".to_string(), rule_lines.join("\n"));
    let rendered = ensure_trailing_newline(
        engine.render_to_string("templates/security/rule_template.nft", &vars)?,
    );
    write_file_atomic(&rule_file, rendered.as_bytes(), Some(0o644))?;

    if !ctx.no_sys {
        let rule_str = rule_file.to_string_lossy();
        let ok = runner
            .run("nft", &["-f", &rule_str], Duration::from_secs(15))
            .map(|o| o.success())
            .unwrap_or(false);
        if !ok {
            let _ = fs::remove_file(&rule_file);
            return Err(CoreError::Security(format!(
                "nftables rule validation failed for {port}/{eff_proto} ({eff_comment})"
            )));
        }
    }

    let include_base_line = format!("include \"{}/*.nft\"", nft_base_d.display());
    if nft_main_conf.is_file() {
        let main_content = fs::read_to_string(&nft_main_conf)?;
        if !main_content.contains(&include_base_line) {
            let mut next = ensure_trailing_newline(main_content);
            next.push_str(&include_base_line);
            next.push('\n');
            write_file_atomic(&nft_main_conf, next.as_bytes(), None)?;
        }
    }

    let mod_entry = nft_base_d.join("50-debopti.nft");
    if !mod_entry.is_file() {
        let mod_content = format!("include \"{}/*.nft\"\n", nft_conf_dir.display());
        write_file_atomic(&mod_entry, mod_content.as_bytes(), Some(0o644))?;
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
    let sanitized = comment.replace(' ', "_");
    let rule_file = ctx
        .effective_nft_conf_dir()
        .join(format!("{sanitized}.nft"));
    if !rule_file.is_file() {
        return Ok(false);
    }

    fs::remove_file(&rule_file)?;

    let nft_main_conf = ctx.effective_nft_main_conf();
    if !ctx.no_sys && nft_main_conf.is_file() {
        let conf_str = nft_main_conf.to_string_lossy();
        let _ = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15));
    }

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(true)
}

/// Idempotently patch legacy `nftables.conf` with IPv6 NDP allow rules after `icmpv6 type echo-request`.
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
                "        # 核心：放行 IPv6 邻居发现协议 (NDP)，防止 IPv6 断网失联".to_string(),
            );
            out_lines.push(
                "        ip6 nexthdr icmpv6 icmpv6 type { nd-router-solicit, nd-router-advert, nd-neighbor-solicit, nd-neighbor-advert } accept"
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

        let err = add_fw_rule(&ctx, "bad", "tcp", "Broken_Rule", &runner).unwrap_err();
        assert!(matches!(err, CoreError::Security(_)));
        assert!(
            !ctx.effective_nft_conf_dir()
                .join("Broken_Rule.nft")
                .exists()
        );
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
