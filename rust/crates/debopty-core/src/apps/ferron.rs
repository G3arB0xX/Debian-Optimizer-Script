use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::HttpClient;
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "ferron";
const DIAG_TARGET: &str = "debopty::apps::ferron";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];
const FERRON_PGP_URL: &str = "https://deb.ferron.sh/signing.pgp";
const FERRON_KEYRING_ABS: &str = "/usr/share/keyrings/ferron-keyring.gpg";
const FERRON_GLOBAL_BLOCK: &str =
    "* {\n    io_uring #false\n    log_stdout\n    error_log_stderr\n}\n\n";

fn resolve_ferron_bin(ctx: &AppContext, runner: &dyn CommandRunner) -> String {
    for candidate in [
        "/usr/bin/ferron",
        "/usr/local/bin/ferron",
        "/usr/sbin/ferron",
    ] {
        if ctx.path(candidate).is_file() {
            return candidate.to_string();
        }
    }
    if !ctx.no_sys
        && !ctx.is_custom_root()
        && let Ok(out) = runner.run("which", &["ferron"], Duration::from_secs(5))
        && out.success()
    {
        let found = out.stdout.trim();
        if !found.is_empty() {
            return found.to_string();
        }
    }
    "/usr/bin/ferron".to_string()
}

/// Deploy Ferron web server from official APT repository with U8 KDL & webroot preservation.
pub fn install_ferron(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting ferron installation",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(
            &[
                "curl",
                "gnupg2",
                "ca-certificates",
                "lsb-release",
                "debian-archive-keyring",
            ],
            runner,
        )?;
    }

    let codename = shared::detect_debian_codename(ctx, runner)?;
    let repo_line =
        format!("deb [signed-by={FERRON_KEYRING_ABS}] https://deb.ferron.sh {codename} main");
    shared::install_apt_keyring_and_repo(
        ctx,
        "ferron",
        FERRON_PGP_URL,
        FERRON_KEYRING_ABS,
        &repo_line,
        http,
        runner,
    )?;

    if !ctx.no_sys {
        let _ = runner.run_with_env(
            "apt-get",
            &["update", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
        sys::safe_apt_install(&["ferron"], runner).map_err(|e| CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("failed to install ferron package for suite '{codename}': {e}"),
        })?;
    }

    let etc_ferron = ctx.path("/etc/ferron");
    fs::create_dir_all(&etc_ferron)?;
    shared::prepare_shared_certs_dir(ctx, runner)?;

    // Migrate legacy /etc/ferron.kdl -> /etc/ferron/config.kdl if target absent
    let legacy_kdl = ctx.path("/etc/ferron.kdl");
    let config_kdl = etc_ferron.join("config.kdl");
    if legacy_kdl.is_file() && !config_kdl.exists() {
        fs::rename(&legacy_kdl, &config_kdl)?;
    }

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        format!("Ferron deployed for Debian {codename}"),
    )
    .with_detail("codename", &codename)
    .with_detail("config_path", config_kdl.to_string_lossy());

    // U8: render default config.kdl only if absent; if present, preserve user routes and prepend io_uring #false once
    if !config_kdl.is_file() {
        ctx.template_engine().render_to_file(
            "templates/apps/ferron/config.kdl",
            &config_kdl,
            &BTreeMap::new(),
            Some(0o644),
        )?;
    } else {
        let existing = fs::read_to_string(&config_kdl)?;
        report = report.with_preserved("/etc/ferron/config.kdl");
        if !existing.contains("io_uring #false") {
            let mut updated = String::with_capacity(FERRON_GLOBAL_BLOCK.len() + existing.len());
            updated.push_str(FERRON_GLOBAL_BLOCK);
            updated.push_str(&existing);
            sys::write_file_atomic(&config_kdl, updated.as_bytes(), Some(0o644))?;
            diag::log_event(
                LogLevel::Info,
                DIAG_TARGET,
                None,
                "prepended io_uring #false block to existing config.kdl",
                None,
                &[("path", &config_kdl.to_string_lossy())],
            );
        }
    }

    // U8: preserve existing /var/www/ferron webroot
    let webroot = ctx.path("/var/www/ferron");
    if !webroot.is_dir() {
        fs::create_dir_all(&webroot)?;
        ctx.template_engine().render_to_file(
            "templates/apps/ferron/index.html",
            &webroot.join("index.html"),
            &BTreeMap::new(),
            Some(0o644),
        )?;
        if !ctx.no_sys && !ctx.is_custom_root() {
            let wr_str = webroot.to_string_lossy();
            let _ = runner.run(
                "chown",
                &["-R", "ferron:ferron", &wr_str],
                Duration::from_secs(15),
            );
        }
    } else {
        report = report.with_preserved("/var/www/ferron");
    }

    let ferron_bin = resolve_ferron_bin(ctx, runner);
    let mut vars = BTreeMap::new();
    vars.insert("FERRON_BIN".to_string(), ferron_bin.clone());
    shared::inject_service_override(
        ctx,
        "ferron",
        "templates/apps/ferron/ferron.service.override.conf",
        &vars,
        runner,
    )?;
    report = report
        .with_detail("ferron_bin", ferron_bin)
        .with_detail("webroot", webroot.to_string_lossy());

    if !ctx.no_sys {
        let active = runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "ferron"],
                Duration::from_secs(15),
            )
            .map(|o| o.success())
            .unwrap_or(false);
        if !active {
            report = report.with_warning("ferron service is not active after installation");
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "ferron installation complete",
        None,
        &[("codename", &codename)],
    );
    Ok(report)
}

/// Purge Ferron package, APT repo/keyring, configs, logs, webroot, and systemd overrides.
pub fn uninstall_ferron(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting ferron uninstallation",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["stop", "ferron"], Duration::from_secs(30));
        let _ = runner.run("systemctl", &["disable", "ferron"], Duration::from_secs(30));
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "ferron"],
            APT_ENV,
            Duration::from_secs(120),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["autoremove", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    for file_abs in [
        "/etc/apt/sources.list.d/ferron.list",
        FERRON_KEYRING_ABS,
        "/etc/ferron.kdl",
    ] {
        let p = ctx.path(file_abs);
        if p.exists() || p.symlink_metadata().is_ok() {
            let _ = fs::remove_file(&p);
        }
    }

    if !ctx.no_sys {
        let _ = runner.run_with_env(
            "apt-get",
            &["update", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    for dir_abs in [
        "/etc/ferron",
        "/var/log/ferron",
        "/var/www/ferron",
        "/etc/systemd/system/ferron.service.d",
    ] {
        let p = ctx.path(dir_abs);
        if p.exists() {
            fs::remove_dir_all(&p)?;
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "ferron uninstallation complete",
        None,
        &[],
    );
    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Ferron removed from system",
    ))
}
