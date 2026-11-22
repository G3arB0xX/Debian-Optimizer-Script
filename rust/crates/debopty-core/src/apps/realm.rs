use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use tempfile::{NamedTempFile, TempDir};
use walkdir::WalkDir;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::CommandRunner;

pub const MODULE_ID: &str = "realm";
const DIAG_TARGET: &str = "debopty::apps::realm";
const REALM_REPO: &str = "zhboner/realm";
const REALM_FALLBACK_VERSION: &str = "v2.7.0";

fn map_realm_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => Err(CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("unsupported architecture for realm: '{other}'"),
        }),
    }
}

/// Deploy or update Realm relay server, preserving any existing `/etc/realm/config.toml` (U8).
pub fn install_realm(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let raw_arch = ctx.arch();
    let target_arch = map_realm_arch(&raw_arch)?;
    let is_cn = ctx.effective_is_cn();

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting realm installation",
        None,
        &[
            ("arch", target_arch),
            ("cn", if is_cn { "true" } else { "false" }),
        ],
    );

    let mut warnings = Vec::new();
    let version = match network::get_latest_github_release(REALM_REPO, is_cn, http) {
        Ok(Some(v)) => v,
        Ok(None) | Err(_) => {
            diag::log_event(
                LogLevel::Warn,
                DIAG_TARGET,
                None,
                "failed to resolve latest realm release; using fallback version",
                None,
                &[("fallback", REALM_FALLBACK_VERSION)],
            );
            warnings.push(format!(
                "GitHub latest release lookup failed; fell back to {REALM_FALLBACK_VERSION}"
            ));
            REALM_FALLBACK_VERSION.to_string()
        }
    };

    let download_url = format!(
        "https://github.com/{REALM_REPO}/releases/download/{version}/realm-{target_arch}.tar.gz"
    );
    let tmp_archive = NamedTempFile::new()?;
    let winning_url =
        network::download_with_fallback(tmp_archive.path(), &download_url, is_cn, http)?;

    let install_dir = ctx.path("/opt/realm");
    fs::create_dir_all(&install_dir)?;

    // BE-P1-15: extract into staging dir and atomically rename over `/opt/realm/realm` to avoid ETXTBSY
    let stage_dir = TempDir::new()?;
    shared::extract_tar_gz(tmp_archive.path(), stage_dir.path(), runner)?;

    let extracted_bin = WalkDir::new(stage_dir.path())
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file() && e.file_name().to_str() == Some("realm"))
        .map(|e| e.into_path());

    let realm_bin = install_dir.join("realm");
    if let Some(src_bin) = extracted_bin {
        shared::install_binary_atomic_from_file(&src_bin, &realm_bin)?;
    } else if !ctx.is_custom_root() {
        return Err(CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!(
                "extracted archive did not contain expected binary at {}",
                realm_bin.display()
            ),
        });
    }

    shared::create_system_user(ctx, "realm", runner)?;

    let etc_dir = ctx.path("/etc/realm");
    fs::create_dir_all(&etc_dir)?;
    let config_file = etc_dir.join("config.toml");

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        format!("Realm {version} deployed ({target_arch})"),
    )
    .with_detail("version", &version)
    .with_detail("arch", target_arch)
    .with_detail("download_url", winning_url)
    .with_detail("config_path", config_file.to_string_lossy());

    for w in warnings {
        report = report.with_warning(w);
    }

    // U8: never overwrite user's existing config.toml
    if config_file.is_file() {
        diag::log_event(
            LogLevel::Info,
            DIAG_TARGET,
            None,
            "preserving existing user realm config.toml",
            None,
            &[("path", &config_file.to_string_lossy())],
        );
        report = report.with_preserved("/etc/realm/config.toml");
    } else {
        ctx.template_engine().render_to_file(
            "templates/apps/realm/config.toml",
            &config_file,
            &BTreeMap::new(),
            Some(0o644),
        )?;
        if !ctx.no_sys && !ctx.is_custom_root() {
            let dir_str = etc_dir.to_string_lossy();
            let _ = runner.run(
                "chown",
                &["-R", "realm:realm", &dir_str],
                Duration::from_secs(15),
            );
        }
    }

    shared::deploy_systemd_service(
        ctx,
        "realm",
        "templates/apps/realm/realm.service",
        &BTreeMap::new(),
        true,
        runner,
    )?;

    if !ctx.no_sys {
        let active = runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "realm"],
                Duration::from_secs(15),
            )
            .map(|o| o.success())
            .unwrap_or(false);
        if !active {
            report = report.with_warning("realm service is not active after installation");
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "realm installation complete",
        None,
        &[("version", &version)],
    );
    Ok(report)
}

/// Remove Realm systemd service, binaries, config directory, and system user.
pub fn uninstall_realm(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting realm uninstallation",
        None,
        &[],
    );

    shared::remove_systemd_service(ctx, "realm", runner)?;

    for dir_abs in ["/opt/realm", "/etc/realm"] {
        let p = ctx.path(dir_abs);
        if p.exists() {
            fs::remove_dir_all(&p)?;
        }
    }

    shared::remove_system_user(ctx, "realm", false, runner)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "realm uninstallation complete",
        None,
        &[],
    );
    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Realm removed from system",
    ))
}
