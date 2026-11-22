use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use tempfile::TempDir;
use walkdir::WalkDir;

use crate::apps::devops::cli_deps;
use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

const DIAG_TARGET: &str = "debopty::apps::devops::micro";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];

const MICRO_PLUGINS: &[(&str, &str)] = &[
    ("MicroOmni", "https://github.com/Neko-Box-Coder/MicroOmni"),
    (
        "gutter_message",
        "https://github.com/usfbih8u/micro-gutter-message",
    ),
    (
        "snippets",
        "https://github.com/micro-editor/updated-plugins.git",
    ),
    ("gitStatus", "https://github.com/Neko-Box-Coder/git-status"),
];

fn micro_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: "micro".to_string(),
        message: message.into(),
    }
}

fn corrupted_goos_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(?:go){2,}os\b").expect("valid corrupted goos regex"))
}

fn broken_mkdir_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:os|goos)\.MkdirAll\(sessionsPath,\s*os\.ModePerm\)")
            .expect("valid micro omni mkdir regex")
    })
}

/// Pure-Rust idempotent patcher for `MicroOmni/Session.lua`.
///
/// 1. Repairs broad-sed corruption (`gogoos`, `gogogoos`, ...) back to `goos`.
/// 2. Patches upstream bug `os.MkdirAll(sessionsPath, os.ModePerm)` to
///    `goos.MkdirAll(sessionsPath, goos.ModePerm)` without corrupting existing `goos.*`.
///
/// Returns `(patched_content, changed)`.
pub fn patch_micro_omni_session_lua(content: &str) -> (String, bool) {
    let step1 = corrupted_goos_re().replace_all(content, "goos");
    let step2 = broken_mkdir_re()
        .replace_all(&step1, "goos.MkdirAll(sessionsPath, goos.ModePerm)")
        .into_owned();
    let changed = step2 != content;
    (step2, changed)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in WalkDir::new(src)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let rel = entry
            .path()
            .strip_prefix(src)
            .map_err(|e| micro_err(format!("strip_prefix failed: {e}")))?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
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

/// Install the Micro terminal editor, OSC52 clipboard wrapper, 4 plugins, MicroOmni patch,
/// environment variables, editor alternative, and SOT symlinks.
pub fn install_micro(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "installing Micro editor and plugin suite",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(&["shellcheck", "yamllint"], runner)?;
    }
    let mut warnings = cli_deps::install_devops_shared_cli_deps(ctx, http, runner)?;

    if !ctx.path("/usr/local/bin/micro").is_file() && !ctx.path("/usr/bin/micro").is_file() {
        let apt_ok = !ctx.no_sys && sys::safe_apt_install(&["micro"], runner).is_ok();
        if !apt_ok {
            let tmp_dir = TempDir::new()?;
            let script_path = tmp_dir.path().join("get_micro.sh");
            network::download_with_fallback(
                &script_path,
                "https://getmic.ro",
                ctx.effective_is_cn(),
                http,
            )
            .map_err(|e| micro_err(format!("failed to download Micro installer: {e}")))?;

            if !ctx.no_sys {
                // BE-P2-13: pass paths via positional arguments to avoid shell injection/word-splitting
                let tmp_dir_str = tmp_dir.path().to_string_lossy().into_owned();
                let script_str = script_path.to_string_lossy().into_owned();
                let out = runner.run(
                    "bash",
                    &[
                        "-c",
                        "cd \"$1\" && bash \"$2\"",
                        "_",
                        &tmp_dir_str,
                        &script_str,
                    ],
                    Duration::from_secs(120),
                )?;
                if !out.success() {
                    return Err(micro_err(format!(
                        "Micro installer script failed (code {}): {}",
                        out.code,
                        out.stderr.trim()
                    )));
                }
                let built_bin = tmp_dir.path().join("micro");
                if built_bin.is_file() {
                    let bytes = fs::read(&built_bin)?;
                    sys::write_file_atomic(&ctx.path("/usr/local/bin/micro"), &bytes, Some(0o755))?;
                }
            }
        }
    }

    let sot_user = ctx.sot_user();
    let sot_home = ctx.user_home(&sot_user);
    let micro_conf = sot_home.join(".config/micro");
    let plug_dir = micro_conf.join("plug");
    fs::create_dir_all(&plug_dir)?;

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();
    engine.render_to_file(
        "templates/apps/devops/micro_settings.json",
        &micro_conf.join("settings.json"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/micro_bindings.json",
        &micro_conf.join("bindings.json"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/micro_init.lua",
        &micro_conf.join("init.lua"),
        &empty_vars,
        Some(0o644),
    )?;

    let osc52_path = ctx.path("/usr/local/bin/debopti-osc52");
    engine.render_to_file(
        "templates/apps/devops/debopti-osc52",
        &osc52_path,
        &empty_vars,
        Some(0o755),
    )?;

    let micro_clip_link = ctx.path("/usr/local/bin/micro-clip");
    remove_path_no_follow(&micro_clip_link);
    let osc52_target = if ctx.is_custom_root() {
        osc52_path
    } else {
        Path::new("/usr/local/bin/debopti-osc52").to_path_buf()
    };
    symlink(&osc52_target, &micro_clip_link)?;

    // Clone 4 Micro plugins
    let is_cn = ctx.effective_is_cn();
    for &(name, repo) in MICRO_PLUGINS {
        if name == "snippets" {
            let tmp_dir = TempDir::new()?;
            let clone_dst = tmp_dir.path().join("micro_updated_plugins");
            match network::git_clone_with_fallback(&clone_dst, repo, &["--depth=1"], is_cn, runner)
            {
                Ok(_) => {
                    let src_sub = clone_dst.join("micro-snippets-plugin");
                    if src_sub.is_dir() {
                        copy_dir_recursive(&src_sub, &plug_dir.join("snippets"))?;
                    }
                }
                Err(_) => {
                    warnings.push("插件 snippets 部署失败，后续可能需要手动安装。".to_string());
                }
            }
        } else if network::git_clone_with_fallback(
            &plug_dir.join(name),
            repo,
            &["--depth=1"],
            is_cn,
            runner,
        )
        .is_err()
        {
            warnings.push(format!("插件 {name} 部署失败，后续可能需要手动安装。"));
        }
    }

    // Patch MicroOmni/Session.lua if present
    let session_lua = plug_dir.join("MicroOmni/Session.lua");
    let mut patched_session = false;
    if session_lua.is_file() {
        let sessions_dir = plug_dir.join("MicroOmni/sessions");
        fs::create_dir_all(&sessions_dir)?;
        let orig = fs::read_to_string(&session_lua)?;
        let (patched, changed) = patch_micro_omni_session_lua(&orig);
        if changed {
            sys::write_file_atomic(&session_lua, patched.as_bytes(), None)?;
            patched_session = true;
            diag::log_event(
                LogLevel::Info,
                DIAG_TARGET,
                None,
                "patched MicroOmni Session.lua",
                None,
                &[("path", &session_lua.to_string_lossy())],
            );
        }
    }

    engine.render_to_file(
        "templates/apps/devops/micro_env.sh",
        &ctx.path("/etc/profile.d/micro_env.sh"),
        &empty_vars,
        Some(0o755),
    )?;
    shared::set_system_env(ctx, "EDITOR", "micro")?;
    shared::set_system_env(ctx, "VISUAL", "micro")?;
    shared::update_fish_env(ctx, "EDITOR", "micro", runner)?;
    shared::update_fish_env(ctx, "VISUAL", "micro", runner)?;

    if !ctx.no_sys {
        let final_bin = if ctx.path("/usr/local/bin/micro").is_file() {
            Some("/usr/local/bin/micro")
        } else if ctx.path("/usr/bin/micro").is_file() {
            Some("/usr/bin/micro")
        } else {
            None
        };
        if let Some(bin) = final_bin {
            let _ = runner.run(
                "update-alternatives",
                &["--install", "/usr/bin/editor", "editor", bin, "100"],
                Duration::from_secs(15),
            );
            let _ = runner.run(
                "update-alternatives",
                &["--set", "editor", bin],
                Duration::from_secs(15),
            );
        } else {
            warnings.push("未能在系统路径中定位到 micro，跳过编辑器替代项配置。".to_string());
        }
    }

    // BE-P1-03: transfer SOT ~/.config/micro ownership back to sot_user after root config/plugin writes
    if sot_user != "root" {
        shared::best_effort_chown(ctx, &sot_user, &micro_conf, true, runner);
    }

    let _ = shared::sync_devops_sot_links(ctx, false, runner)?;

    let mut report = AppReport::new("micro", "install", true, "Micro editor deployed")
        .with_detail("sot_user", sot_user)
        .with_detail("micro_omni_patched", patched_session.to_string());
    for w in warnings {
        report = report.with_warning(w);
    }
    Ok(report)
}

/// Uninstall Micro editor, alternatives, OSC52 wrapper, environment variables, and SOT links.
pub fn uninstall_micro(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Micro editor",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run(
            "update-alternatives",
            &["--remove", "editor", "/usr/local/bin/micro"],
            Duration::from_secs(15),
        );
        let _ = runner.run(
            "update-alternatives",
            &["--remove", "editor", "/usr/bin/micro"],
            Duration::from_secs(15),
        );
    }

    // BE-P1-25: do NOT remove shared `/usr/local/bin/bat` or purge shared `xclip` (used by Yazi/Tmux/Fish)
    for p in [
        "/usr/local/bin/micro",
        "/usr/local/bin/debopti-osc52",
        "/usr/local/bin/micro-clip",
        "/etc/profile.d/micro_env.sh",
    ] {
        remove_path_no_follow(&ctx.path(p));
    }

    if !ctx.no_sys {
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "micro"],
            APT_ENV,
            Duration::from_secs(180),
        );
    }

    let sot_user = ctx.sot_user();
    let all_users = ctx.all_real_users();
    for u in &all_users {
        if u == &sot_user {
            continue;
        }
        // U8: only unlink managed symlinks for non-SOT users; never delete custom user config dirs
        let user_micro = ctx.user_home(u).join(".config/micro");
        if let Ok(meta) = fs::symlink_metadata(&user_micro)
            && meta.file_type().is_symlink()
        {
            let _ = fs::remove_file(&user_micro);
        }
    }
    remove_path_no_follow(&ctx.user_home(&sot_user).join(".config/micro"));

    for key in ["MICRO_TRUECOLOR", "EDITOR", "VISUAL"] {
        shared::remove_system_env(ctx, key)?;
        shared::remove_fish_env(ctx, key, runner)?;
    }

    Ok(AppReport::new(
        "micro",
        "uninstall",
        true,
        "Micro editor uninstalled",
    ))
}
