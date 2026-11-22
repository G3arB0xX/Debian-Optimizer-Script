use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use tempfile::TempDir;

use crate::apps::devops::cli_deps;
use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

const DIAG_TARGET: &str = "debopty::apps::devops::fish";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];

fn fish_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: "fish".to_string(),
        message: message.into(),
    }
}

fn remove_path_no_follow(path: &Path) {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || meta.is_file() {
            let _ = fs::remove_file(path);
        } else if meta.is_dir() {
            let _ = fs::remove_dir_all(path);
        }
    }
}

fn dir_has_any_entry(path: &Path) -> bool {
    fs::read_dir(path)
        .ok()
        .and_then(|mut rd| rd.next())
        .is_some()
}

/// Deploy the `y` Yazi CWD-sync wrapper into `/etc/fish/conf.d/yazi.fish` and SOT `functions/y.fish`.
pub fn deploy_fish_yazi_wrapper(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<bool> {
    if !ctx.path("/usr/local/bin/yazi").is_file() {
        return Ok(false);
    }
    if !ctx.is_cmd_present("fish", runner) && !ctx.path("/etc/fish").is_dir() {
        return Ok(false);
    }

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();

    engine.render_to_file(
        "templates/apps/devops/y.fish",
        &ctx.path("/etc/fish/conf.d/yazi.fish"),
        &empty_vars,
        Some(0o644),
    )?;

    let sot_user = ctx.sot_user();
    let sot_fish = ctx.user_home(&sot_user).join(".config/fish");
    if sot_fish.is_dir() {
        let funcs_dir = sot_fish.join("functions");
        fs::create_dir_all(&funcs_dir)?;
        let y_dest = funcs_dir.join("y.fish");
        engine.render_to_file(
            "templates/apps/devops/y.fish",
            &y_dest,
            &empty_vars,
            Some(0o644),
        )?;
        shared::best_effort_chown(ctx, &sot_user, &y_dest, false, runner);
    }

    Ok(true)
}

/// Install Fish shell, Starship prompt, Fisher plugins on SOT user, and hybrid SOT links.
pub fn install_fish(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "installing Fish shell and DevOps SOT environment",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(&["fish", "curl", "git"], runner)?;
    }

    let mut warnings = cli_deps::install_devops_shared_cli_deps(ctx, http, runner)?;

    // Install Starship if missing
    if !ctx.path("/usr/local/bin/starship").is_file() && !ctx.path("/usr/bin/starship").is_file() {
        let apt_ok = !ctx.no_sys && sys::safe_apt_install(&["starship"], runner).is_ok();
        if !apt_ok {
            let tmp_dir = TempDir::new()?;
            let script_path = tmp_dir.path().join("starship_install.sh");
            network::download_with_fallback(
                &script_path,
                "https://starship.rs/install.sh",
                ctx.effective_is_cn(),
                http,
            )
            .map_err(|e| fish_err(format!("failed to download Starship installer: {e}")))?;

            if !ctx.no_sys {
                let s = script_path.to_string_lossy().into_owned();
                let out = runner.run("sh", &[&s, "-y"], Duration::from_secs(120))?;
                if !out.success() {
                    return Err(fish_err(format!(
                        "Starship installer failed (code {}): {}",
                        out.code,
                        out.stderr.trim()
                    )));
                }
            }
        }
    }

    let sot_user = ctx.sot_user();
    let sot_home = ctx.user_home(&sot_user);
    let fish_conf_dir = sot_home.join(".config/fish");
    let functions_dir = fish_conf_dir.join("functions");
    let conf_d_dir = fish_conf_dir.join("conf.d");
    fs::create_dir_all(&functions_dir)?;
    fs::create_dir_all(&conf_d_dir)?;

    // BE-P1-03: transfer SOT ~/.config/fish ownership before running fisher as non-root sot_user
    if sot_user != "root" {
        shared::best_effort_chown(ctx, &sot_user, &fish_conf_dir, true, runner);
    }

    // Install Fisher + core plugins on SOT user
    if !functions_dir.join("fisher.fish").is_file() {
        let tmp_dir = TempDir::new()?;
        let tmp_fisher = tmp_dir.path().join("fisher.fish");
        if network::download_with_fallback(
            &tmp_fisher,
            "https://raw.githubusercontent.com/jorgebucaran/fisher/main/functions/fisher.fish",
            ctx.effective_is_cn(),
            http,
        )
        .is_ok()
        {
            // BE-P1-03: TempDir is created root:root 0700; make readable/owned by sot_user before `fish -c`
            let _ = fs::set_permissions(tmp_dir.path(), fs::Permissions::from_mode(0o755));
            let _ = fs::set_permissions(&tmp_fisher, fs::Permissions::from_mode(0o644));
            if sot_user != "root" {
                shared::best_effort_chown(ctx, &sot_user, tmp_dir.path(), true, runner);
            }
            if !ctx.no_sys {
                let fisher_str = tmp_fisher.to_string_lossy().into_owned();
                let _ = shared::run_as_user(
                    ctx,
                    &sot_user,
                    "fish",
                    &[
                        "-c",
                        "source $argv[1] && fisher install jorgebucaran/fisher",
                        &fisher_str,
                    ],
                    &[],
                    Duration::from_secs(120),
                    runner,
                );
            }
        } else {
            warnings.push("Fisher 下载失败，插件管理器可能未自动部署。".to_string());
        }
    }

    if !ctx.no_sys {
        let _ = shared::run_as_user(
            ctx,
            &sot_user,
            "fish",
            &[
                "-c",
                "fisher install PatrickF1/fzf.fish jorgebucaran/autopair.fish nickeb96/puffer-fish jorgebucaran/replay.fish",
            ],
            &[],
            Duration::from_secs(180),
            runner,
        );
    }

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();

    // Fish 3.x puffer-fish compatibility override
    engine.render_to_file(
        "templates/apps/devops/puffer_fish_expand_dot.fish",
        &functions_dir.join("_puffer_fish_expand_dot.fish"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/zoxide.fish",
        &conf_d_dir.join("zoxide.fish"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/starship.fish",
        &conf_d_dir.join("starship.fish"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/abbrs.fish",
        &conf_d_dir.join("abbrs.fish"),
        &empty_vars,
        Some(0o644),
    )?;

    // Global Starship Gruvbox-Rainbow preset
    let starship_toml = ctx.path("/etc/starship.toml");
    if !ctx.no_sys && ctx.is_cmd_present("starship", runner) {
        let starship_bin = if ctx.path("/usr/local/bin/starship").is_file() {
            "/usr/local/bin/starship"
        } else {
            "starship"
        };
        let out_str = starship_toml.to_string_lossy().into_owned();
        let _ = runner.run(
            starship_bin,
            &["preset", "gruvbox-rainbow", "-o", &out_str],
            Duration::from_secs(30),
        );
        if starship_toml.is_file() {
            let _ = fs::set_permissions(&starship_toml, fs::Permissions::from_mode(0o644));
        }
    }

    // Set default shell to Fish for all real users
    let all_users = ctx.all_real_users();
    if !ctx.no_sys {
        let fish_path = if ctx.path("/usr/bin/fish").is_file() {
            "/usr/bin/fish"
        } else if ctx.path("/usr/local/bin/fish").is_file() {
            "/usr/local/bin/fish"
        } else {
            "/usr/bin/fish"
        };
        for u in &all_users {
            let _ = runner.run("chsh", &["-s", fish_path, u], Duration::from_secs(15));
        }
    }

    let _ = deploy_fish_yazi_wrapper(ctx, runner)?;
    // BE-P1-03: ensure SOT user owns all root-rendered Fish config files before SOT link sync
    if sot_user != "root" {
        shared::best_effort_chown(ctx, &sot_user, &fish_conf_dir, true, runner);
    }
    let _ = shared::sync_devops_sot_links(ctx, false, runner)?;

    let mut report = AppReport::new(
        "fish",
        "install",
        true,
        "Fish shell and hybrid SOT environment deployed",
    )
    .with_detail("sot_user", sot_user)
    .with_detail("users_configured", all_users.len().to_string());

    for w in warnings {
        report = report.with_warning(w);
    }
    Ok(report)
}

/// Remove managed symlinks & bootstrap from a non-SOT user's `~/.config/fish` WITHOUT
/// traversing into the SOT directory and while preserving `conf.d.local/` and non-symlink
/// user custom files in `conf.d/` (`U8`).
pub fn clean_non_sot_fish_dir(fish_dir: &Path) -> Result<Vec<String>> {
    let mut preserved = Vec::new();
    let Ok(meta) = fs::symlink_metadata(fish_dir) else {
        return Ok(preserved);
    };

    // Legacy whole-dir symlink: unlink ONLY the symlink itself; never traverse into SOT!
    if meta.file_type().is_symlink() {
        fs::remove_file(fish_dir)?;
        return Ok(preserved);
    }
    if !meta.is_dir() {
        return Ok(preserved);
    }

    // Remove managed symlinks and debopti bootstrap inside conf.d, preserving user regular files
    let conf_d = fish_dir.join("conf.d");
    if let Ok(conf_meta) = fs::symlink_metadata(&conf_d) {
        if conf_meta.file_type().is_symlink() {
            fs::remove_file(&conf_d)?;
        } else if conf_meta.is_dir() {
            for entry in fs::read_dir(&conf_d)? {
                let entry = entry?;
                let p = entry.path();
                let emeta = fs::symlink_metadata(&p)?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if emeta.file_type().is_symlink() || name == "00-debopti-sot-bootstrap.fish" {
                    let _ = fs::remove_file(&p);
                } else {
                    preserved.push(format!("preserved user Fish file {}", p.display()));
                }
            }
            if !dir_has_any_entry(&conf_d) {
                let _ = fs::remove_dir(&conf_d);
            }
        }
    }

    // Inspect remaining entries in fish_dir; preserve conf.d.local if non-empty
    for entry in fs::read_dir(fish_dir)? {
        let entry = entry?;
        let p = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "conf.d" {
            continue;
        }
        let emeta = fs::symlink_metadata(&p)?;
        if name == "conf.d.local" && !emeta.file_type().is_symlink() && emeta.is_dir() {
            if dir_has_any_entry(&p) {
                preserved.push(format!("preserved user Fish directory {}", p.display()));
            } else {
                let _ = fs::remove_dir(&p);
            }
            continue;
        }
        if emeta.file_type().is_symlink() || emeta.is_file() {
            let _ = fs::remove_file(&p);
        } else if emeta.is_dir() && !dir_has_any_entry(&p) {
            let _ = fs::remove_dir(&p);
        }
    }

    if !dir_has_any_entry(fish_dir) {
        let _ = fs::remove_dir(fish_dir);
    }

    Ok(preserved)
}

/// Uninstall Fish shell, restore `/bin/bash`, and cleanly remove managed SOT links
/// without deleting SOT targets via symlinks and preserving non-SOT `conf.d.local/` (`U8`).
pub fn uninstall_fish(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Fish shell and cleaning hybrid SOT links",
        None,
        &[],
    );

    let all_users = ctx.all_real_users();
    if !ctx.no_sys {
        for u in &all_users {
            let _ = runner.run("chsh", &["-s", "/bin/bash", u], Duration::from_secs(15));
        }
        // BE-P1-25: do NOT purge shared `zoxide` (also used by Yazi/Tmux/users)
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "fish"],
            APT_ENV,
            Duration::from_secs(180),
        );
    }

    remove_path_no_follow(&ctx.path("/usr/local/bin/starship"));

    let sot_user = ctx.sot_user();
    let mut preserved = Vec::new();

    // Clean non-SOT users first so symlinks are unlinked before touching SOT
    for u in &all_users {
        if u == &sot_user {
            continue;
        }
        let u_home = ctx.user_home(u);
        let fish_dir = u_home.join(".config/fish");
        preserved.extend(clean_non_sot_fish_dir(&fish_dir)?);
        remove_path_no_follow(&u_home.join(".config/starship.toml"));
    }

    // Clean SOT user's Fish directory (preserving non-empty conf.d.local if present)
    let sot_home = ctx.user_home(&sot_user);
    let sot_fish = sot_home.join(".config/fish");
    if let Ok(meta) = fs::symlink_metadata(&sot_fish) {
        if meta.file_type().is_symlink() {
            fs::remove_file(&sot_fish)?;
        } else if meta.is_dir() {
            let local_dir = sot_fish.join("conf.d.local");
            let keep_local = local_dir.is_dir()
                && !fs::symlink_metadata(&local_dir)
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false)
                && dir_has_any_entry(&local_dir);
            if keep_local {
                for entry in fs::read_dir(&sot_fish)? {
                    let entry = entry?;
                    if entry.file_name().to_str() == Some("conf.d.local") {
                        continue;
                    }
                    remove_path_no_follow(&entry.path());
                }
                preserved.push(format!(
                    "preserved SOT user Fish directory {}",
                    local_dir.display()
                ));
            } else {
                fs::remove_dir_all(&sot_fish)?;
            }
        }
    }
    remove_path_no_follow(&sot_home.join(".config/starship.toml"));

    remove_path_no_follow(&ctx.path("/etc/fish/shared_sot"));
    remove_path_no_follow(&ctx.path("/etc/starship.toml"));
    remove_path_no_follow(&ctx.path("/etc/debopti/sot_known_users"));

    let mut report = AppReport::new("fish", "uninstall", true, "Fish shell uninstalled");
    for p in preserved {
        report = report.with_preserved(p);
    }
    Ok(report)
}
