use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::security::firewall;
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "easytier";
const DIAG_TARGET: &str = "debopty::apps::easytier";
const EASYTIER_INSTALL_URL: &str =
    "https://raw.githubusercontent.com/EasyTier/EasyTier/main/script/install.sh";
const CN_GH_PROXIES: &[&str] = &[
    "https://gh-proxy.com/",
    "https://edgeone.gh-proxy.org/",
    "https://gh-proxy.org/",
    "https://ghfast.top/",
];

/// Install or update EasyTier mesh VPN, preserving enabled instances on update (U8) and syncing firewall rules.
pub fn install_easytier(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let is_cn = ctx.effective_is_cn();
    let is_fresh = !ctx.path("/opt/easytier/easytier-core").is_file();
    let action = if is_fresh { "install" } else { "update" };

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting easytier deployment",
        None,
        &[
            ("mode", action),
            ("cn", if is_cn { "true" } else { "false" }),
        ],
    );

    if !ctx.no_sys && !ctx.is_cmd_present("unzip", runner) {
        sys::safe_apt_install(&["unzip"], runner).map_err(|e| CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("failed to install unzip dependency: {e}"),
        })?;
    }

    let tmp_script = NamedTempFile::new()?.into_temp_path();
    network::download_with_fallback(&tmp_script, EASYTIER_INSTALL_URL, is_cn, http)?;
    let script_str = tmp_script.to_string_lossy().into_owned();

    let mut warnings = Vec::new();
    if !ctx.no_sys {
        if is_cn {
            let mut succeeded = false;
            for proxy in CN_GH_PROXIES {
                let res = runner.run(
                    "bash",
                    &[&script_str, action, "--gh-proxy", proxy],
                    Duration::from_secs(300),
                );
                if let Ok(out) = res
                    && out.success()
                {
                    succeeded = true;
                    break;
                }
                diag::log_event(
                    LogLevel::Warn,
                    DIAG_TARGET,
                    None,
                    "easytier CN mirror failed; trying next proxy",
                    None,
                    &[("proxy", proxy), ("mode", action)],
                );
                warnings.push(format!("EasyTier {action} failed via proxy {proxy}"));
            }
            if !succeeded {
                return Err(CoreError::App {
                    app: MODULE_ID.to_string(),
                    message: format!("easytier {action} failed across all CN GitHub proxies"),
                });
            }
        } else {
            let out = runner.run(
                "bash",
                &[&script_str, action, "--no-gh-proxy"],
                Duration::from_secs(300),
            )?;
            if !out.success() {
                return Err(CoreError::App {
                    app: MODULE_ID.to_string(),
                    message: format!(
                        "easytier {action} failed (code {}): {}",
                        out.code,
                        out.stderr.trim()
                    ),
                });
            }
        }
    }

    let mut report = AppReport::new(
        MODULE_ID,
        action,
        true,
        format!("EasyTier {action} completed"),
    )
    .with_detail("mode", action)
    .with_detail("fw_rule", "Easytier_P2P");

    for w in warnings {
        report = report.with_warning(w);
    }

    // U8: disable default auto-start ONLY on fresh install; update preserves user instances
    if is_fresh {
        if !ctx.no_sys {
            let _ = runner.run(
                "systemctl",
                &["stop", "easytier", "easytier@default"],
                Duration::from_secs(30),
            );
            let _ = runner.run(
                "systemctl",
                &["stop", "easytier@*"],
                Duration::from_secs(30),
            );
            let _ = runner.run(
                "systemctl",
                &["disable", "easytier"],
                Duration::from_secs(30),
            );
            let _ = runner.run(
                "systemctl",
                &["disable", "easytier@default"],
                Duration::from_secs(30),
            );
            let _ = runner.run(
                "systemctl",
                &["disable", "easytier@*"],
                Duration::from_secs(30),
            );
        }
    } else {
        report = report.with_preserved("existing easytier@ systemd instances");
    }

    let profile_path = ctx.path("/etc/profile.d/debopti-easytier.sh");
    ctx.template_engine().render_to_file(
        "templates/apps/easytier/profile.d/debopti-easytier.sh",
        &profile_path,
        &BTreeMap::new(),
        Some(0o644),
    )?;
    shared::update_fish_path(ctx, "/opt/easytier", runner)?;

    shared::inject_service_override(
        ctx,
        "easytier@",
        "templates/apps/easytier/easytier@.service.override.conf",
        &BTreeMap::new(),
        runner,
    )?;

    firewall::add_fw_rule(
        &ctx.security_context(),
        "11010-11015",
        "tcp/udp",
        "Easytier_P2P",
        runner,
    )?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "easytier deployment complete",
        None,
        &[("mode", action)],
    );
    Ok(report)
}

/// Remove EasyTier binaries, systemd units/overrides, PATH entries, and firewall rule.
pub fn uninstall_easytier(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting easytier uninstallation",
        None,
        &[],
    );

    let mut report = AppReport::new(MODULE_ID, "uninstall", true, "EasyTier removed from system");

    // Best-effort upstream uninstall script (must succeed even when offline)
    if !ctx.no_sys {
        if let Ok(tmp) = NamedTempFile::new() {
            let tmp_path = tmp.into_temp_path();
            if network::download_with_fallback(
                &tmp_path,
                EASYTIER_INSTALL_URL,
                ctx.effective_is_cn(),
                http,
            )
            .is_ok()
            {
                let s = tmp_path.to_string_lossy().into_owned();
                let _ = runner.run("bash", &[&s, "uninstall"], Duration::from_secs(120));
            } else {
                report = report.with_warning(
                    "upstream easytier uninstall script unavailable offline; continuing local cleanup",
                );
            }
        }
        let _ = runner.run("systemctl", &["stop", "easytier"], Duration::from_secs(30));
        let _ = runner.run(
            "systemctl",
            &["disable", "easytier"],
            Duration::from_secs(30),
        );
    }

    let systemd_dir = ctx.path("/etc/systemd/system");
    if systemd_dir.is_dir()
        && let Ok(entries) = fs::read_dir(&systemd_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("easytier") {
                let p = entry.path();
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    let profile_path = ctx.path("/etc/profile.d/debopti-easytier.sh");
    if profile_path.exists() || profile_path.symlink_metadata().is_ok() {
        let _ = fs::remove_file(&profile_path);
    }
    shared::remove_fish_path(ctx, "/opt/easytier", runner)?;

    for file_abs in [
        "/usr/bin/easytier-core",
        "/usr/bin/easytier-cli",
        "/usr/local/bin/easytier-core",
        "/usr/sbin/easytier-core",
        "/usr/sbin/easytier-cli",
    ] {
        let p = ctx.path(file_abs);
        if p.exists() || p.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&p);
        }
    }

    for dir_abs in ["/opt/easytier", "/opt/easytier-core"] {
        let p = ctx.path(dir_abs);
        if p.exists() {
            let _ = fs::remove_dir_all(&p);
        }
    }

    firewall::remove_fw_rule(&ctx.security_context(), "Easytier_P2P", runner)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "easytier uninstallation complete",
        None,
        &[],
    );
    Ok(report)
}
