use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::security::firewall;
use crate::sys::CommandRunner;

pub const MODULE_ID: &str = "tailscale";
const DIAG_TARGET: &str = "debopty::apps::tailscale";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];
const TAILSCALE_INSTALL_URL: &str = "https://tailscale.com/install.sh";

/// Deploy or update Tailscale, inject systemd sandbox override, and open UDP 41641 in nftables.
pub fn install_tailscale(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let is_update = ctx.is_cmd_present("tailscale", runner);
    let mode = if is_update { "update" } else { "install" };

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting tailscale deployment",
        None,
        &[("mode", mode)],
    );

    let tmp_script = NamedTempFile::new()?.into_temp_path();
    network::download_with_fallback(
        &tmp_script,
        TAILSCALE_INSTALL_URL,
        ctx.effective_is_cn(),
        http,
    )?;

    if !ctx.no_sys {
        let script_str = tmp_script.to_string_lossy().into_owned();
        let out = runner.run("sh", &[&script_str], Duration::from_secs(300))?;
        if !out.success() {
            return Err(CoreError::App {
                app: MODULE_ID.to_string(),
                message: format!(
                    "tailscale installer failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                ),
            });
        }
    }

    shared::inject_service_override(
        ctx,
        "tailscaled",
        "templates/apps/tailscale/tailscaled.service.override.conf",
        &BTreeMap::new(),
        runner,
    )?;

    firewall::add_fw_rule(
        &ctx.security_context(),
        "41641",
        "udp",
        "Tailscale_P2P",
        runner,
    )?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "tailscale deployment complete",
        None,
        &[("mode", mode)],
    );

    Ok(
        AppReport::new(MODULE_ID, mode, true, format!("Tailscale {mode} completed"))
            .with_detail("mode", mode)
            .with_detail("fw_rule", "Tailscale_P2P"),
    )
}

/// Purge Tailscale package, state directories, systemd overrides, and firewall rule.
pub fn uninstall_tailscale(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting tailscale uninstallation",
        None,
        &[],
    );

    if !ctx.no_sys {
        // BE-P2-12: stop and disable tailscaled before purging the package
        let _ = runner.run(
            "systemctl",
            &["stop", "tailscaled"],
            Duration::from_secs(30),
        );
        let _ = runner.run(
            "systemctl",
            &["disable", "tailscaled"],
            Duration::from_secs(30),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "tailscale"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    for file_abs in [
        "/usr/bin/tailscale",
        "/usr/sbin/tailscaled",
        "/etc/apt/sources.list.d/tailscale.list",
        "/usr/share/keyrings/tailscale-archive-keyring.gpg",
    ] {
        let p = ctx.path(file_abs);
        if p.exists() || p.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&p);
        }
    }

    for dir_abs in [
        "/var/lib/tailscale",
        "/etc/tailscale",
        "/opt/tailscale",
        "/etc/systemd/system/tailscaled.service.d",
    ] {
        let p = ctx.path(dir_abs);
        if p.exists() {
            fs::remove_dir_all(&p)?;
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    firewall::remove_fw_rule(&ctx.security_context(), "Tailscale_P2P", runner)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "tailscale uninstallation complete",
        None,
        &[],
    );
    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Tailscale removed from system",
    ))
}
