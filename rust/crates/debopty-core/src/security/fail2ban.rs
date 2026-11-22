use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use tempfile::NamedTempFile;

use crate::error::{CoreError, Result};
use crate::security::f2b_sync::{self, F2bSyncReport};
use crate::security::firewall;
use crate::security::{ensure_trailing_newline, write_file_atomic};
use crate::sys::{self, CommandRunner};

pub use crate::security::SecurityContext;

const LEGACY_JAIL_SIGNATURES: &[&str] = &[
    "banaction = nftables-multiport",
    "banaction_allports = nftables-allports",
    "logpath = /var/log/auth.log",
    "bantime = 86400",
    "findtime = 600",
];

fn ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([0-9]+)\.([0-9]+)").expect("valid version regex"))
}

fn parse_major_minor(text: &str) -> Option<(u32, u32)> {
    let caps = ver_re().captures(text)?;
    let major = caps[1].parse::<u32>().ok()?;
    let minor = caps[2].parse::<u32>().ok()?;
    Some((major, minor))
}

/// Detect whether installed Fail2ban version is `>= 0.11` (defaults to `true` when undetectable).
pub fn detect_fail2ban_version_ge_011(runner: &dyn CommandRunner) -> bool {
    let mut parsed: Option<(u32, u32)> = None;

    if let Ok(out) = runner.run("fail2ban-client", &["--version"], Duration::from_secs(5))
        && out.success()
    {
        parsed = parse_major_minor(&out.stdout).or_else(|| parse_major_minor(&out.stderr));
    }

    if parsed.is_none()
        && let Ok(out) = runner.run(
            "dpkg-query",
            &["-W", "-f=${Version}\n", "fail2ban"],
            Duration::from_secs(5),
        )
        && out.success()
    {
        parsed = parse_major_minor(&out.stdout);
    }

    match parsed {
        Some((major, minor)) => major > 0 || (major == 0 && minor >= 11),
        None => true,
    }
}

/// Detect Fail2ban log backend: `"systemd"` when systemd + `python3-systemd` are available, else `"auto"`.
pub fn detect_fail2ban_backend(ctx: &SecurityContext, runner: &dyn CommandRunner) -> String {
    if let Some(ref ov) = ctx.f2b_backend_override
        && !ov.is_empty()
    {
        return ov.clone();
    }

    if ctx.path("/run/systemd/system").is_dir() {
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
            return "systemd".to_string();
        }
    }

    "auto".to_string()
}

/// Remove the legacy 12-line auto-generated `/etc/fail2ban/jail.local` while preserving any customized file.
///
/// Requires `line_count <= 15` AND all 5 exact legacy signature lines.
pub fn cleanup_legacy_fail2ban_jail_local(ctx: &SecurityContext) -> Result<bool> {
    let legacy_file = ctx.path("/etc/fail2ban/jail.local");
    if !legacy_file.is_file() {
        return Ok(false);
    }

    let content = fs::read_to_string(&legacy_file)?;
    let lines: Vec<&str> = content.lines().map(|l| l.trim_end_matches('\r')).collect();
    if lines.len() <= 15 && LEGACY_JAIL_SIGNATURES.iter().all(|sig| lines.contains(sig)) {
        fs::remove_file(&legacy_file)?;
        return Ok(true);
    }

    Ok(false)
}

/// Detect active SSH port across `ssh.socket.d/*.conf` (`override.conf`), `sshd_config.d/*.conf`,
/// `sshd_config` (case-insensitive, above `Match` blocks), and `sshd -T`, falling back to `22`.
pub fn detect_ssh_port(ctx: &SecurityContext, runner: &dyn CommandRunner) -> u16 {
    f2b_sync::detect_ssh_port_from_configs(&ctx.root_prefix, ctx.no_sys, runner)
}

/// Deploy Fail2ban best-practice jails, public-port filter, sync helper, systemd units, and run native sync.
pub fn setup_fail2ban(
    ctx: &SecurityContext,
    ssh_port: Option<u16>,
    runner: &dyn CommandRunner,
) -> Result<F2bSyncReport> {
    let port = ssh_port.unwrap_or_else(|| detect_ssh_port(ctx, runner));

    if !ctx.no_sys {
        sys::safe_apt_install(&["fail2ban"], runner)?;
        let _ = sys::safe_apt_install(&["python3-systemd"], runner);
    }

    let f2b_backend = detect_fail2ban_backend(ctx, runner);
    let ge_011 = detect_fail2ban_version_ge_011(runner);

    let (f2b_default_bantime, f2b_sshd_bantime, f2b_increment_block) = if ge_011 {
        (
            "1h",
            "1h",
            "bantime.increment = true\nbantime.factor = 2\nbantime.maxtime = 7d\nbantime.rndtime = 5m",
        )
    } else {
        (
            "24h",
            "24h",
            "# Incremental bantime requires Fail2ban >= 0.11",
        )
    };

    let sshd_log_directive = if f2b_backend == "systemd" {
        "backend = systemd"
    } else {
        "logpath = /var/log/auth.log"
    };

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();

    // 1. Core best-practice jail & public-port filter
    let mut jail_vars = BTreeMap::new();
    jail_vars.insert("F2B_BACKEND".to_string(), f2b_backend.clone());
    jail_vars.insert(
        "F2B_DEFAULT_BANTIME".to_string(),
        f2b_default_bantime.to_string(),
    );
    jail_vars.insert("F2B_SSHD_BANTIME".to_string(), f2b_sshd_bantime.to_string());
    jail_vars.insert(
        "F2B_INCREMENT_BLOCK".to_string(),
        f2b_increment_block.to_string(),
    );
    jail_vars.insert("SSH_PORT".to_string(), port.to_string());
    jail_vars.insert(
        "SSHD_LOG_DIRECTIVE".to_string(),
        sshd_log_directive.to_string(),
    );

    let jail_rendered = ensure_trailing_newline(
        engine.render_to_string("templates/security/fail2ban_jail.local", &jail_vars)?,
    );
    write_file_atomic(
        &ctx.path("/etc/fail2ban/jail.d/99-debopty.local"),
        jail_rendered.as_bytes(),
        Some(0o644),
    )?;

    let filter_rendered = ensure_trailing_newline(engine.render_to_string(
        "templates/security/fail2ban_filter_public_ports.conf",
        &empty_vars,
    )?);
    write_file_atomic(
        &ctx.path("/etc/fail2ban/filter.d/debopty-public-ports.conf"),
        filter_rendered.as_bytes(),
        Some(0o644),
    )?;

    cleanup_legacy_fail2ban_jail_local(ctx)?;

    // 2. Deploy standalone sync helper + runtime templates
    let sync_bin_dest = ctx.path("/usr/local/bin/debopty-f2b-sync");
    if let Some(parent) = sync_bin_dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(ref bin_path) = ctx.binary_path {
        if sync_bin_dest.exists() || sync_bin_dest.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&sync_bin_dest);
        }
        symlink(bin_path, &sync_bin_dest)?;
    } else {
        let sync_script = engine.get_raw("templates/security/debopty-f2b-sync.sh")?;
        write_file_atomic(&sync_bin_dest, sync_script.as_bytes(), Some(0o755))?;
    }

    for tmpl_name in ["fail2ban_ports.local", "f2b_public_ports_guard.nft"] {
        let rel = format!("templates/security/{tmpl_name}");
        let raw = engine.get_raw(&rel)?;
        let dest = ctx.path(&format!("/usr/local/share/debopty/{rel}"));
        write_file_atomic(&dest, raw.as_bytes(), Some(0o644))?;
    }

    // 3. Deploy systemd units & nftables reload override
    let unit_mappings = [
        (
            "templates/security/debopty-f2b-port-sync.service",
            "/etc/systemd/system/debopty-f2b-port-sync.service",
        ),
        (
            "templates/security/debopty-f2b-port-sync.path",
            "/etc/systemd/system/debopty-f2b-port-sync.path",
        ),
        (
            "templates/security/debopty-f2b-port-sync.timer",
            "/etc/systemd/system/debopty-f2b-port-sync.timer",
        ),
        (
            "templates/security/nftables-fail2ban-override.conf",
            "/etc/systemd/system/nftables.service.d/10-debopty-fail2ban.conf",
        ),
    ];
    for (tmpl, dest_abs) in unit_mappings {
        let rendered = ensure_trailing_newline(engine.render_to_string(tmpl, &empty_vars)?);
        write_file_atomic(&ctx.path(dest_abs), rendered.as_bytes(), Some(0o644))?;
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
        let _ = runner.run(
            "systemctl",
            &["enable", "fail2ban"],
            Duration::from_secs(15),
        );
        let _ = runner.run(
            "systemctl",
            &["restart", "fail2ban"],
            Duration::from_secs(15),
        );
        let _ = runner.run(
            "systemctl",
            &[
                "enable",
                "--now",
                "debopty-f2b-port-sync.path",
                "debopty-f2b-port-sync.timer",
            ],
            Duration::from_secs(15),
        );
    }

    // 4. Native Fail2ban public-port sync
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)
}

/// Initialize the full nftables + Fail2ban security engine (`scripts/security.sh::setup_security`).
pub fn setup_security(ctx: &SecurityContext, runner: &dyn CommandRunner) -> Result<F2bSyncReport> {
    setup_security_with_port(ctx, None, runner)
}

/// Initialize the full nftables + Fail2ban security engine with an optional explicit SSH port.
///
/// When `ssh_port` is `Some(port)`, that port is enforced in `nftables.conf` and Fail2ban
/// instead of re-probing (preventing SSH lockout during port transitions).
pub fn setup_security_with_port(
    ctx: &SecurityContext,
    ssh_port: Option<u16>,
    runner: &dyn CommandRunner,
) -> Result<F2bSyncReport> {
    let mut ufw_present = false;
    if !ctx.no_sys {
        ufw_present = runner
            .run("dpkg", &["-s", "ufw"], Duration::from_secs(10))
            .map(|o| o.success())
            .unwrap_or(false);
        sys::safe_apt_install(&["nftables", "fail2ban"], runner)?;
    }

    let ssh_port = ssh_port.unwrap_or_else(|| detect_ssh_port(ctx, runner));
    if ssh_port == 0 {
        return Err(CoreError::ValidationFailed(
            "SSH port must be in 1..=65535".to_string(),
        ));
    }
    firewall::migrate_nft_paths(ctx)?;

    let nft_base_d = ctx.effective_nft_base_d();
    let nft_conf_dir = ctx.effective_nft_conf_dir();
    let nft_main_conf = ctx.effective_nft_main_conf();

    fs::create_dir_all(&nft_conf_dir)?;
    fs::create_dir_all(&nft_base_d)?;

    let engine = ctx.template_engine();
    let mut nft_vars = BTreeMap::new();
    nft_vars.insert("SSH_PORT".to_string(), ssh_port.to_string());
    nft_vars.insert(
        "NFT_BASE_D".to_string(),
        nft_base_d.to_string_lossy().into_owned(),
    );
    let rendered_nft = ensure_trailing_newline(
        engine.render_to_string("templates/security/nftables.conf", &nft_vars)?,
    );

    let mod_entry = nft_base_d.join("50-debopti.nft");
    let prev_nft_main = fs::read(&nft_main_conf).ok();
    let prev_mod_entry = fs::read(&mod_entry).ok();

    let mod_content = format!("include \"{}/*.nft\"\n", nft_conf_dir.display());
    write_file_atomic(&mod_entry, mod_content.as_bytes(), Some(0o644))?;

    let restore_mod = |prev: Option<&[u8]>| {
        if let Some(bytes) = prev {
            let _ = write_file_atomic(&mod_entry, bytes, Some(0o644));
        } else {
            let _ = fs::remove_file(&mod_entry);
        }
    };

    if !ctx.no_sys {
        // Stage and dry-run validate with `nft -c -f` BEFORE overwriting `/etc/nftables.conf`.
        let parent = nft_main_conf
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        let mut staged = NamedTempFile::new_in(parent)?;
        staged.write_all(rendered_nft.as_bytes())?;
        staged.flush()?;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o644))?;

        let staged_str = staged.path().to_string_lossy().into_owned();
        let check = runner.run("nft", &["-c", "-f", &staged_str], Duration::from_secs(15))?;
        if !check.success() {
            restore_mod(prev_mod_entry.as_deref());
            return Err(CoreError::Security(format!(
                "nftables dry-run validation failed for {}: {}",
                nft_main_conf.display(),
                check.stderr.trim()
            )));
        }

        write_file_atomic(&nft_main_conf, rendered_nft.as_bytes(), Some(0o644))?;
        let _ = runner.run(
            "systemctl",
            &["enable", "--now", "nftables"],
            Duration::from_secs(15),
        );
        let conf_str = nft_main_conf.to_string_lossy();
        let load = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15))?;
        if !load.success() {
            if let Some(ref bytes) = prev_nft_main {
                let _ = write_file_atomic(&nft_main_conf, bytes, Some(0o644));
            } else {
                let _ = fs::remove_file(&nft_main_conf);
            }
            restore_mod(prev_mod_entry.as_deref());
            return Err(CoreError::Security(format!(
                "nftables load failed for {}: {}",
                nft_main_conf.display(),
                load.stderr.trim()
            )));
        }

        // Only disable/purge UFW AFTER nftables has been validated and loaded cleanly.
        if ufw_present {
            let _ = runner.run("ufw", &["disable"], Duration::from_secs(15));
            let _ = runner.run_with_env(
                "apt-get",
                &["purge", "-yq", "ufw"],
                &[("DEBIAN_FRONTEND", "noninteractive")],
                Duration::from_secs(120),
            );
        }
    } else {
        write_file_atomic(&nft_main_conf, rendered_nft.as_bytes(), Some(0o644))?;
    }

    setup_fail2ban(ctx, Some(ssh_port), runner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn ok_stdout(s: &str) -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: s.to_string(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_out() -> CmdOutput {
        CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    #[test]
    fn fail2ban_version_and_backend_detection() {
        let runner = MockCommandRunner::new();
        runner.on_exact(
            "fail2ban-client",
            &["--version"],
            ok_stdout("Fail2Ban v1.0.2\n"),
        );
        assert!(detect_fail2ban_version_ge_011(&runner));

        let runner_old = MockCommandRunner::new();
        runner_old.on_exact(
            "fail2ban-client",
            &["--version"],
            ok_stdout("Fail2Ban v0.10.4\n"),
        );
        assert!(!detect_fail2ban_version_ge_011(&runner_old));

        let runner_dpkg = MockCommandRunner::new();
        runner_dpkg.on_exact("fail2ban-client", &["--version"], err_out());
        runner_dpkg.on_exact(
            "dpkg-query",
            &["-W", "-f=${Version}\n", "fail2ban"],
            ok_stdout("0.11.2-6\n"),
        );
        assert!(detect_fail2ban_version_ge_011(&runner_dpkg));

        // Backend detection with /run/systemd/system fixture
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        let runner_auto = MockCommandRunner::new();
        assert_eq!(detect_fail2ban_backend(&ctx, &runner_auto), "auto");

        fs::create_dir_all(ctx.path("/run/systemd/system")).unwrap();
        runner_auto.on_exact("python3", &["-c", "import systemd.journal"], ok_stdout(""));
        assert_eq!(detect_fail2ban_backend(&ctx, &runner_auto), "systemd");
    }

    #[test]
    fn cleanup_legacy_jail_local_removes_signature_and_preserves_custom() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        let jail = ctx.path("/etc/fail2ban/jail.local");
        fs::create_dir_all(jail.parent().unwrap()).unwrap();

        let legacy = "\
[DEFAULT]
banaction = nftables-multiport
banaction_allports = nftables-allports

[sshd]
enabled = true
port = 22
filter = sshd
logpath = /var/log/auth.log
maxretry = 3
bantime = 86400
findtime = 600
";
        fs::write(&jail, legacy).unwrap();
        assert!(cleanup_legacy_fail2ban_jail_local(&ctx).unwrap());
        assert!(!jail.exists());

        // Custom file must NOT be removed
        fs::write(&jail, "[DEFAULT]\nignoreip = 10.0.0.1\n").unwrap();
        assert!(!cleanup_legacy_fail2ban_jail_local(&ctx).unwrap());
        assert!(jail.is_file());
    }

    #[test]
    fn setup_security_and_fail2ban_deploys_all_artifacts() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = false;
        ctx.f2b_backend_override = Some("systemd".to_string());

        let runner = MockCommandRunner::new();
        runner.on_exact("dpkg", &["-s", "ufw"], ok_stdout("installed"));
        runner.on_exact("dpkg", &["-s", "nftables"], ok_stdout("installed"));
        runner.on_exact("dpkg", &["-s", "fail2ban"], ok_stdout("installed"));
        runner.on_exact("dpkg", &["-s", "python3-systemd"], ok_stdout("installed"));
        runner.on_exact(
            "sshd",
            &["-T"],
            ok_stdout("port 2222\npermitrootlogin no\n"),
        );
        runner.on_exact(
            "fail2ban-client",
            &["--version"],
            ok_stdout("Fail2Ban v1.0.2\n"),
        );
        runner.on_prog("ufw", ok_stdout(""));
        runner.on_prog("apt-get", ok_stdout(""));
        runner.on_prog("systemctl", ok_stdout(""));
        runner.on_prog("nft", ok_stdout(""));

        let report = setup_security(&ctx, &runner).unwrap();
        assert_eq!(report.ssh_port, 2222);

        let nft_main = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_main.contains(r#"tcp dport 2222 accept comment "SSH_Access_Port""#));
        assert!(ctx.effective_nft_base_d().join("50-debopti.nft").is_file());

        let jail_main =
            fs::read_to_string(ctx.path("/etc/fail2ban/jail.d/99-debopty.local")).unwrap();
        assert!(jail_main.contains("port = 2222"));
        assert!(jail_main.contains("bantime.increment = true"));
        assert!(jail_main.contains("backend = systemd"));

        assert!(
            ctx.path("/etc/fail2ban/filter.d/debopty-public-ports.conf")
                .is_file()
        );
        assert!(
            ctx.path("/etc/systemd/system/debopty-f2b-port-sync.service")
                .is_file()
        );
        assert!(
            ctx.path("/etc/systemd/system/debopty-f2b-port-sync.path")
                .is_file()
        );
        assert!(
            ctx.path("/etc/systemd/system/debopty-f2b-port-sync.timer")
                .is_file()
        );
        assert!(
            ctx.path("/etc/systemd/system/nftables.service.d/10-debopty-fail2ban.conf")
                .is_file()
        );
        assert!(
            ctx.path("/usr/local/share/debopty/templates/security/fail2ban_ports.local")
                .is_file()
        );
        assert!(
            ctx.path("/usr/local/share/debopty/templates/security/f2b_public_ports_guard.nft")
                .is_file()
        );

        let sync_bin = ctx.path("/usr/local/bin/debopty-f2b-sync");
        assert!(
            sync_bin
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_link(&sync_bin).unwrap(),
            std::env::current_exe().unwrap()
        );

        // Verify explicit fallback script mode when binary_path is None
        ctx.binary_path = None;
        setup_fail2ban(&ctx, Some(2222), &runner).unwrap();
        assert!(sync_bin.is_file());
        assert!(
            !sync_bin
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&sync_bin).unwrap().permissions().mode() & 0o777,
            0o755
        );

        // Verify symlink mode when explicit binary_path is provided
        let fake_bin = tmp.path().join("debopty-bin");
        fs::write(&fake_bin, b"#!/bin/sh\n").unwrap();
        ctx.binary_path = Some(fake_bin.clone());
        setup_fail2ban(&ctx, Some(2222), &runner).unwrap();
        assert_eq!(fs::read_link(&sync_bin).unwrap(), fake_bin);
    }

    #[test]
    fn detect_ssh_port_prefers_ssh_socket_override_over_sshd_t() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = false;
        ctx.f2b_backend_override = Some("systemd".to_string());

        let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        fs::write(
            &override_path,
            "[Socket]\nListenStream=\nListenStream=51234\n",
        )
        .unwrap();

        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        fs::create_dir_all(sshd_conf.parent().unwrap()).unwrap();
        fs::write(&sshd_conf, "#Port 22\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("sshd", &["-T"], ok_stdout("port 22\n"));
        runner.on_exact("dpkg", &["-s", "ufw"], err_out());
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "nftables"],
            ok_stdout("install ok installed"),
        );
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "fail2ban"],
            ok_stdout("install ok installed"),
        );
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "python3-systemd"],
            ok_stdout("install ok installed"),
        );
        runner.on_exact(
            "fail2ban-client",
            &["--version"],
            ok_stdout("Fail2Ban v1.0.2\n"),
        );
        runner.on_prog("systemctl", ok_stdout(""));
        runner.on_prog("nft", ok_stdout(""));

        assert_eq!(detect_ssh_port(&ctx, &runner), 51234);

        let report = setup_security(&ctx, &runner).unwrap();
        assert_eq!(report.ssh_port, 51234);
        let nft_main = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_main.contains(r#"tcp dport 51234 accept comment "SSH_Access_Port""#));
    }

    #[test]
    fn detect_ssh_port_handles_sshd_config_d_lowercase_and_ignores_match_block() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.no_sys = true;
        let runner = MockCommandRunner::new();

        // Lowercase port in sshd_config; Port inside Match block ignored
        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        fs::create_dir_all(sshd_conf.parent().unwrap()).unwrap();
        fs::write(&sshd_conf, "Match User foo\n    Port 9999\n").unwrap();
        assert_eq!(detect_ssh_port(&ctx, &runner), 22);

        fs::write(&sshd_conf, "port 2244\nMatch User foo\n    Port 9999\n").unwrap();
        assert_eq!(detect_ssh_port(&ctx, &runner), 2244);

        // sshd_config.d drop-in takes precedence over sshd_config
        let dropin = ctx.path("/etc/ssh/sshd_config.d/10-port.conf");
        fs::create_dir_all(dropin.parent().unwrap()).unwrap();
        fs::write(&dropin, "Port 2299\n").unwrap();
        assert_eq!(detect_ssh_port(&ctx, &runner), 2299);
    }
}
