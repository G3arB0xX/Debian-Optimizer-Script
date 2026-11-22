use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use walkdir::WalkDir;

use crate::apps::devops::{cli_deps, fish};
use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

const DIAG_TARGET: &str = "debopty::apps::devops::yazi";
const YAZI_FALLBACK_TAG: &str = "v0.4.0";
const YAZI_BASHRC_MARKER: &str = "# Yazi CWD Sync Wrapper";

/// Options for `install_yazi`.
/// When `media_preview` is `None`, reuses persisted `YAZI_MEDIA_PREVIEW` or defaults to `false`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct YaziInstallOptions {
    pub media_preview: Option<bool>,
}

fn yazi_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: "yazi".to_string(),
        message: message.into(),
    }
}

fn find_file_named(root: &Path, filename: &str) -> Option<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file() && e.file_name().to_str() == Some(filename))
        .map(|e| e.into_path())
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

/// Idempotently append `snippet` to `/etc/bash.bashrc` content if `# Yazi CWD Sync Wrapper` is absent.
pub fn inject_yazi_bashrc(existing: &str, snippet: &str) -> (String, bool) {
    if existing.contains(YAZI_BASHRC_MARKER) {
        return (existing.to_string(), false);
    }
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(snippet);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    (out, true)
}

/// Surgically remove ONLY the `# Yazi CWD Sync Wrapper` block from `/etc/bash.bashrc`,
/// preserving 100% of unrelated user lines (`U8`).
pub fn remove_yazi_bashrc(existing: &str) -> (String, bool) {
    if !existing.contains(YAZI_BASHRC_MARKER) {
        return (existing.to_string(), false);
    }

    let lines: Vec<&str> = existing.lines().collect();
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut removed_any = false;

    while i < lines.len() {
        if lines[i].trim() == YAZI_BASHRC_MARKER {
            removed_any = true;
            // Remove one preceding blank separator line if we added it
            if kept.last().is_some_and(|l| l.trim().is_empty()) {
                kept.pop();
            }
            i += 1;
            // Skip the 3-line wrapper block (`if ... yazi_wrapper.sh`, `. ...`, `fi`)
            if i < lines.len() && lines[i].contains("yazi_wrapper.sh") {
                i += 1;
                while i < lines.len() {
                    let trimmed = lines[i].trim();
                    i += 1;
                    if trimmed == "fi" {
                        break;
                    }
                }
            }
        } else {
            kept.push(lines[i]);
            i += 1;
        }
    }

    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    (out, removed_any)
}

fn resolve_media_preview(ctx: &AppContext, opts: &YaziInstallOptions) -> Result<bool> {
    let store = ctx.config_store();
    if let Some(explicit) = opts.media_preview {
        store.set(
            "YAZI_MEDIA_PREVIEW",
            if explicit { "true" } else { "false" },
        )?;
        return Ok(explicit);
    }

    if let Some(persisted) = store.get("YAZI_MEDIA_PREVIEW")? {
        let trimmed = persisted.trim();
        if trimmed == "true" {
            return Ok(true);
        }
        if trimmed == "false" {
            return Ok(false);
        }
    }

    // Non-interactive default is false, persisted for idempotent re-runs
    store.set("YAZI_MEDIA_PREVIEW", "false")?;
    Ok(false)
}

/// Install the Yazi terminal file manager, shared & optional media dependencies,
/// SOT configuration templates, Fish/Bash CWD sync wrappers, and official plugins.
pub fn install_yazi(
    ctx: &AppContext,
    opts: &YaziInstallOptions,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "installing Yazi file manager",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(&["unzip", "file", "jq", "p7zip-full"], runner)?;
    }

    let mut warnings = cli_deps::install_devops_shared_cli_deps(ctx, http, runner)?;
    let media_preview = resolve_media_preview(ctx, opts)?;
    if media_preview {
        if !ctx.no_sys
            && sys::safe_apt_install(&["ffmpeg", "poppler-utils", "imagemagick"], runner).is_err()
        {
            warnings.push("部分多媒体依赖安装失败，相关预览可能不可用".to_string());
        }
        let _ = cli_deps::ensure_resvg(ctx, http, runner, &mut warnings)?;
    }

    let is_cn = ctx.effective_is_cn();
    let tag = match network::get_latest_github_release("sxyazi/yazi", is_cn, http)
        .ok()
        .flatten()
    {
        Some(t)
            if t.strip_prefix('v')
                .unwrap_or(&t)
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()) =>
        {
            t
        }
        _ => {
            warnings.push(format!(
                "无法从 GitHub 获取 Yazi 最新版本，使用默认兜底版本: {YAZI_FALLBACK_TAG}"
            ));
            YAZI_FALLBACK_TAG.to_string()
        }
    };

    let arch = ctx.arch();
    let asset_name = match arch.as_str() {
        "x86_64" => "yazi-x86_64-unknown-linux-musl.zip",
        "aarch64" => "yazi-aarch64-unknown-linux-musl.zip",
        _ => {
            return Err(yazi_err(format!(
                "unsupported architecture for Yazi: {arch}"
            )));
        }
    };

    let url = format!("https://github.com/sxyazi/yazi/releases/download/{tag}/{asset_name}");
    let tmp_dir = TempDir::new()?;
    let tmp_zip = tmp_dir.path().join(format!("yazi_{tag}.zip"));
    let extract_dir = tmp_dir.path().join("extract");

    network::download_with_fallback(&tmp_zip, &url, is_cn, http)
        .map_err(|e| yazi_err(format!("failed to download Yazi release archive: {e}")))?;
    shared::extract_zip(&tmp_zip, &extract_dir, runner)
        .map_err(|e| yazi_err(format!("failed to extract Yazi zip archive: {e}")))?;

    let bin_yazi = find_file_named(&extract_dir, "yazi")
        .ok_or_else(|| yazi_err("extracted Yazi archive missing 'yazi' binary"))?;
    let bin_ya = find_file_named(&extract_dir, "ya")
        .ok_or_else(|| yazi_err("extracted Yazi archive missing 'ya' binary"))?;

    let yazi_dest = ctx.path("/usr/local/bin/yazi");
    let ya_dest = ctx.path("/usr/local/bin/ya");
    sys::write_file_atomic(&yazi_dest, &fs::read(&bin_yazi)?, Some(0o755))?;
    sys::write_file_atomic(&ya_dest, &fs::read(&bin_ya)?, Some(0o755))?;

    // SOT configs
    let sot_user = ctx.sot_user();
    let sot_home = ctx.user_home(&sot_user);
    let sot_yazi_conf = sot_home.join(".config/yazi");
    fs::create_dir_all(&sot_yazi_conf)?;

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();
    engine.render_to_file(
        "templates/apps/devops/yazi.toml",
        &sot_yazi_conf.join("yazi.toml"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/yazi_keymap.toml",
        &sot_yazi_conf.join("keymap.toml"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/devops/yazi_init.lua",
        &sot_yazi_conf.join("init.lua"),
        &empty_vars,
        Some(0o644),
    )?;

    let _ = fish::deploy_fish_yazi_wrapper(ctx, runner)?;

    engine.render_to_file(
        "templates/apps/devops/yazi_wrapper.sh",
        &ctx.path("/etc/profile.d/yazi_wrapper.sh"),
        &empty_vars,
        Some(0o755),
    )?;

    // Idempotent /etc/bash.bashrc injection from Template SSOT (`10 §2`, `U8`)
    let bashrc_path = ctx.path("/etc/bash.bashrc");
    let existing_bashrc = if bashrc_path.is_file() {
        fs::read_to_string(&bashrc_path)?
    } else {
        String::new()
    };
    let snippet =
        engine.render_to_string("templates/apps/devops/yazi_bashrc_snippet.sh", &empty_vars)?;
    let (next_bashrc, injected) = inject_yazi_bashrc(&existing_bashrc, &snippet);
    if injected {
        sys::write_file_atomic(&bashrc_path, next_bashrc.as_bytes(), None)?;
    }

    // BE-P1-03: chown SOT ~/.config/yazi to sot_user BEFORE running `ya pkg add` as non-root sot_user
    if sot_user != "root" {
        shared::best_effort_chown(ctx, &sot_user, &sot_yazi_conf, true, runner);
    }

    // Best-effort Yazi official plugins
    if !ctx.no_sys && ya_dest.is_file() {
        let ya_cmd = if ctx.is_custom_root() {
            ya_dest.to_string_lossy().into_owned()
        } else {
            "/usr/local/bin/ya".to_string()
        };
        for plugin in [
            "yazi-rs/plugins:git",
            "yazi-rs/plugins:chmod",
            "yazi-rs/plugins:max-preview",
        ] {
            let _ = shared::run_as_user(
                ctx,
                &sot_user,
                &ya_cmd,
                &["pkg", "add", plugin],
                &[],
                Duration::from_secs(60),
                runner,
            );
        }
    }

    let _ = shared::sync_devops_sot_links(ctx, false, runner)?;

    let mut report = AppReport::new("yazi", "install", true, "Yazi file manager deployed")
        .with_detail("version", tag)
        .with_detail("sot_user", sot_user)
        .with_detail("media_preview", media_preview.to_string())
        .with_detail("bashrc_injected", injected.to_string());
    for w in warnings {
        report = report.with_warning(w);
    }
    Ok(report)
}

/// Uninstall Yazi while surgically removing ONLY the managed snippet from `/etc/bash.bashrc` (`U8`).
pub fn uninstall_yazi(ctx: &AppContext, _runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Yazi file manager",
        None,
        &[],
    );

    for p in [
        "/usr/local/bin/yazi",
        "/usr/local/bin/ya",
        "/etc/profile.d/yazi_wrapper.sh",
        "/etc/fish/conf.d/yazi.fish",
    ] {
        remove_path_no_follow(&ctx.path(p));
    }

    let sot_user = ctx.sot_user();
    let sot_home = ctx.user_home(&sot_user);
    remove_path_no_follow(&sot_home.join(".config/fish/functions/y.fish"));

    let mut report = AppReport::new("yazi", "uninstall", true, "Yazi file manager uninstalled");

    let bashrc_path = ctx.path("/etc/bash.bashrc");
    if bashrc_path.is_file() {
        let existing = fs::read_to_string(&bashrc_path)?;
        let (cleaned, removed) = remove_yazi_bashrc(&existing);
        if removed {
            sys::write_file_atomic(&bashrc_path, cleaned.as_bytes(), None)?;
            if !cleaned.trim().is_empty() {
                report =
                    report.with_preserved("preserved unrelated user entries in /etc/bash.bashrc");
            }
        }
    }

    let all_users = ctx.all_real_users();
    for u in &all_users {
        if u == &sot_user {
            continue;
        }
        // U8: only unlink managed symlinks for non-SOT users; never delete custom user config dirs
        let user_yazi = ctx.user_home(u).join(".config/yazi");
        if let Ok(meta) = fs::symlink_metadata(&user_yazi)
            && meta.file_type().is_symlink()
        {
            let _ = fs::remove_file(&user_yazi);
        }
    }
    remove_path_no_follow(&sot_home.join(".config/yazi"));

    Ok(report)
}
