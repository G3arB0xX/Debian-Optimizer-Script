use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::hardware::AdaptiveTuningProfile;
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "rust";
const DIAG_TARGET: &str = "debopty::apps::rust";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];
const BUILD_ESSENTIAL_KEY: &str = "RUST_INSTALLED_BUILD_ESSENTIAL";
const CN_RUSTUP_DIST: &str = "https://rsproxy.cn";
const CN_RUSTUP_UPDATE: &str = "https://rsproxy.cn/rustup";

fn map_rustup_arch(arch: &str) -> Result<&'static str> {
    match arch {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        other => Err(CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("unsupported architecture for rustup: '{other}'"),
        }),
    }
}

/// Deploy Rust toolchain via rustup for `initial_user`, tracking `build-essential` ownership and preserving custom Cargo config (U8).
pub fn install_rust(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let raw_arch = ctx.arch();
    let triple = map_rustup_arch(&raw_arch)?;
    let is_cn = ctx.effective_is_cn();
    let target_user = ctx.initial_user();
    let user_home = ctx.user_home(&target_user);

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting rust toolchain installation",
        None,
        &[
            ("user", &target_user),
            ("arch", triple),
            ("cn", if is_cn { "true" } else { "false" }),
        ],
    );

    if !ctx.no_sys && !sys::is_dpkg_installed("build-essential", runner) {
        sys::safe_apt_install(&["build-essential"], runner).map_err(|e| CoreError::App {
            app: MODULE_ID.to_string(),
            message: format!("failed to install build-essential: {e}"),
        })?;
        ctx.config_store().set(BUILD_ESSENTIAL_KEY, "true")?;
    }

    let download_url = if is_cn {
        format!("https://rsproxy.cn/rustup/dist/{triple}/rustup-init")
    } else {
        format!("https://static.rust-lang.org/rustup/dist/{triple}/rustup-init")
    };

    // Convert to TempPath so write fd is closed before execution (avoids ETXTBSY)
    let tmp_init = NamedTempFile::new()?.into_temp_path();
    network::download_with_fallback(&tmp_init, &download_url, is_cn, http)?;
    fs::set_permissions(&tmp_init, fs::Permissions::from_mode(0o755))?;

    let home_str = user_home.to_string_lossy().into_owned();
    let init_str = tmp_init.to_string_lossy().into_owned();
    let hw_profile = AdaptiveTuningProfile::probe_for_root(
        &ctx.root_prefix,
        &ctx.config_store().load().unwrap_or_default(),
    );
    let build_jobs = hw_profile.build_jobs.to_string();
    let mut env_vars: Vec<(&str, &str)> =
        vec![("HOME", &home_str), ("CARGO_BUILD_JOBS", &build_jobs)];
    if is_cn {
        env_vars.push(("RUSTUP_DIST_SERVER", CN_RUSTUP_DIST));
        env_vars.push(("RUSTUP_UPDATE_ROOT", CN_RUSTUP_UPDATE));
    }

    if !ctx.no_sys {
        let out = shared::run_as_user(
            ctx,
            &target_user,
            &init_str,
            &[
                "-y",
                "--default-toolchain",
                "stable",
                "--profile",
                "minimal",
                "--no-modify-path",
            ],
            &env_vars,
            Duration::from_secs(300),
            runner,
        )?;
        if !out.success() {
            return Err(CoreError::App {
                app: MODULE_ID.to_string(),
                message: format!(
                    "rustup-init failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                ),
            });
        }
    }

    let cargo_dir = user_home.join(".cargo");
    fs::create_dir_all(&cargo_dir)?;

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        format!("Rust toolchain installed for {target_user}"),
    )
    .with_detail("target_user", &target_user)
    .with_detail("user_home", &home_str)
    .with_detail("cn_mirror", if is_cn { "true" } else { "false" })
    .with_detail("build_jobs", &build_jobs);

    // U8: only write CN cargo config if absent or already rsproxy-managed
    if is_cn {
        let cargo_cfg = cargo_dir.join("config.toml");
        if cargo_cfg.is_file() {
            let existing = fs::read_to_string(&cargo_cfg)?;
            if existing.contains("rsproxy") {
                ctx.template_engine().render_to_file(
                    "templates/apps/rust/cargo_config_cn.toml",
                    &cargo_cfg,
                    &BTreeMap::new(),
                    Some(0o644),
                )?;
            } else {
                diag::log_event(
                    LogLevel::Warn,
                    DIAG_TARGET,
                    None,
                    "preserving custom user Cargo config.toml without rsproxy mirror",
                    None,
                    &[("path", &cargo_cfg.to_string_lossy())],
                );
                report = report
                    .with_preserved(cargo_cfg.to_string_lossy())
                    .with_warning("existing custom .cargo/config.toml preserved; CN rsproxy mirror not overwritten");
            }
        } else {
            ctx.template_engine().render_to_file(
                "templates/apps/rust/cargo_config_cn.toml",
                &cargo_cfg,
                &BTreeMap::new(),
                Some(0o644),
            )?;
        }
    }

    if target_user != "root" && !ctx.no_sys {
        shared::best_effort_chown(ctx, &target_user, &cargo_dir, true, runner);
        let rustup_dir = user_home.join(".rustup");
        if rustup_dir.exists() || !ctx.is_custom_root() {
            shared::best_effort_chown(ctx, &target_user, &rustup_dir, true, runner);
        }
    }

    shared::update_fish_path(ctx, "$HOME/.cargo/bin", runner)?;

    let rustc_ver = if !ctx.no_sys {
        let rustc_bin = cargo_dir.join("bin/rustc");
        let rustc_str = rustc_bin.to_string_lossy().into_owned();
        shared::run_as_user(
            ctx,
            &target_user,
            &rustc_str,
            &["--version"],
            &[("HOME", &home_str)],
            Duration::from_secs(15),
            runner,
        )
        .ok()
        .filter(|o| o.success())
        .and_then(|o| o.stdout.split_whitespace().nth(1).map(String::from))
        .unwrap_or_else(|| "unknown".to_string())
    } else {
        "unknown".to_string()
    };
    report = report.with_detail("rustc_version", &rustc_ver);

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "rust toolchain installation complete",
        None,
        &[("rustc_version", &rustc_ver)],
    );
    Ok(report)
}

/// Uninstall Rust toolchain and purge `build-essential` ONLY if installed by this module (U8).
pub fn uninstall_rust(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    let target_user = ctx.initial_user();
    let user_home = ctx.user_home(&target_user);

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting rust toolchain uninstallation",
        None,
        &[("user", &target_user)],
    );

    if target_user != "root" {
        if !ctx.no_sys {
            let home_str = user_home.to_string_lossy().into_owned();
            // BE-P1-11: resolve $HOME/.cargo/bin/rustup directly because sudo secure_path excludes it
            let user_rustup = user_home.join(".cargo/bin/rustup");
            let rustup_cmd = if user_rustup.is_file() {
                user_rustup.to_string_lossy().into_owned()
            } else {
                "rustup".to_string()
            };
            let _ = shared::run_as_user(
                ctx,
                &target_user,
                &rustup_cmd,
                &["self", "uninstall", "-y"],
                &[("HOME", &home_str)],
                Duration::from_secs(60),
                runner,
            );
        }
        for dir in [user_home.join(".cargo"), user_home.join(".rustup")] {
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }

    if ctx.euid() == 0 {
        let root_home = ctx.user_home("root");
        if !ctx.no_sys {
            let root_str = root_home.to_string_lossy().into_owned();
            let root_rustup = root_home.join(".cargo/bin/rustup");
            let rustup_cmd = if root_rustup.is_file() {
                root_rustup.to_string_lossy().into_owned()
            } else {
                "rustup".to_string()
            };
            let _ = shared::run_as_user(
                ctx,
                "root",
                &rustup_cmd,
                &["self", "uninstall", "-y"],
                &[("HOME", &root_str)],
                Duration::from_secs(60),
                runner,
            );
        }
        for dir in [root_home.join(".cargo"), root_home.join(".rustup")] {
            if dir.exists() {
                let _ = fs::remove_dir_all(&dir);
            }
        }
    }

    let mut report = AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Rust toolchain removed from system",
    );

    let store = ctx.config_store();
    let owned_build_essential = store
        .get(BUILD_ESSENTIAL_KEY)?
        .is_some_and(|v| v.trim() == "true");
    if owned_build_essential {
        if !ctx.no_sys {
            let _ = runner.run_with_env(
                "apt-get",
                &["purge", "-yq", "build-essential"],
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
        store.set(BUILD_ESSENTIAL_KEY, "false")?;
        report = report.with_detail("purged_build_essential", "true");
    } else {
        report = report.with_preserved("build-essential");
    }

    shared::remove_fish_path(ctx, "$HOME/.cargo/bin", runner)?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "rust toolchain uninstallation complete",
        None,
        &[],
    );
    Ok(report)
}
