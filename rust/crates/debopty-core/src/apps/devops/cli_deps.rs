use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use walkdir::WalkDir;

use crate::apps::shared::{self, AppContext};
use crate::diag::{self, LogLevel};
use crate::error::Result;
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

const DIAG_TARGET: &str = "debopty::apps::devops::cli_deps";
const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];

pub const FZF_MIN_VERSION: &str = "0.53";
pub const FZF_FALLBACK_TAG: &str = "v0.73.1";
pub const FD_FALLBACK_TAG: &str = "v10.2.0";
pub const RIPGREP_FALLBACK_TAG: &str = "v14.1.1";
pub const NERD_FONT_FALLBACK_TAG: &str = "v3.4.0";
pub const RESVG_FALLBACK_TAG: &str = "v0.47.0";

fn apt_pkg_available(pkg: &str, runner: &dyn CommandRunner) -> bool {
    runner
        .run("apt-cache", &["show", pkg], Duration::from_secs(15))
        .map(|o| o.success())
        .unwrap_or(false)
}

fn github_linux_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        "armv7l" | "armhf" | "arm" => Some("armv7"),
        "i386" | "i686" | "x86" => Some("386"),
        _ => None,
    }
}

fn github_dpkg_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("amd64"),
        "aarch64" => Some("arm64"),
        "armv7l" | "armhf" | "arm" => Some("armhf"),
        "i386" | "i686" | "x86" => Some("i386"),
        _ => None,
    }
}

fn fd_target_triple(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" => Some("x86_64-unknown-linux-gnu"),
        "aarch64" => Some("aarch64-unknown-linux-gnu"),
        "armv7l" | "armhf" | "arm" => Some("armv7-unknown-linux-gnueabihf"),
        "i386" | "i686" | "x86" => Some("i686-unknown-linux-gnu"),
        _ => None,
    }
}

fn valid_tag_or(tag: Option<String>, fallback: &str) -> String {
    match tag {
        Some(t)
            if t.strip_prefix('v')
                .unwrap_or(&t)
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_digit()) =>
        {
            t
        }
        _ => fallback.to_string(),
    }
}

fn find_file_named(root: &Path, filename: &str) -> Option<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file() && e.file_name().to_str() == Some(filename))
        .map(|e| e.into_path())
}

fn install_binary_atomic(src: &Path, dest: &Path) -> Result<()> {
    let bytes = fs::read(src)?;
    sys::write_file_atomic(dest, &bytes, Some(0o755))
}

fn resolve_cmd_path(ctx: &AppContext, cmd: &str) -> PathBuf {
    for dir in ["/usr/local/bin", "/usr/bin", "/usr/sbin", "/bin"] {
        let abs = format!("{dir}/{cmd}");
        let p = ctx.path(&abs);
        if p.is_file() {
            return if ctx.is_custom_root() {
                p
            } else {
                PathBuf::from(abs)
            };
        }
    }
    PathBuf::from(format!("/usr/bin/{cmd}"))
}

fn replace_symlink(target: &Path, link_path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(link_path) {
        if meta.file_type().is_symlink() || meta.is_file() {
            fs::remove_file(link_path)?;
        } else if meta.is_dir() {
            fs::remove_dir_all(link_path)?;
        }
    }
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink(target, link_path)?;
    Ok(())
}

fn detect_fzf_version(runner: &dyn CommandRunner) -> Option<String> {
    runner
        .run("fzf", &["--version"], Duration::from_secs(10))
        .ok()
        .filter(|o| o.success())
        .and_then(|o| o.stdout.split_whitespace().next().map(ToString::to_string))
}

/// Ensure `fzf >= 0.53` is installed (upgrades via GitHub Release when missing or below threshold).
/// Returns `Ok(true)` if installed/upgraded, `Ok(false)` if already satisfied or skipped.
pub fn ensure_fzf(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("fzf", runner) {
        if let Some(cur) = detect_fzf_version(runner)
            && shared::compare_dot_version_ge(&cur, FZF_MIN_VERSION)
        {
            diag::log_event(
                LogLevel::Debug,
                DIAG_TARGET,
                None,
                "fzf version already satisfies minimum threshold",
                None,
                &[("current", &cur), ("minimum", FZF_MIN_VERSION)],
            );
            return Ok(false);
        }
        diag::log_event(
            LogLevel::Info,
            DIAG_TARGET,
            None,
            "fzf below minimum version; upgrading",
            None,
            &[("minimum", FZF_MIN_VERSION)],
        );
    }

    if !ctx.no_sys && apt_pkg_available("fzf", runner) {
        let _ = sys::safe_apt_install(&["fzf"], runner);
        if ctx.is_cmd_present("fzf", runner)
            && let Some(apt_ver) = detect_fzf_version(runner)
            && shared::compare_dot_version_ge(&apt_ver, FZF_MIN_VERSION)
        {
            return Ok(true);
        }
    }

    let arch = ctx.arch();
    let Some(arch_name) = github_linux_arch(&arch) else {
        warnings.push(format!("fzf: 不支持的架构 {arch}"));
        return Ok(false);
    };

    let is_cn = ctx.effective_is_cn();
    let tag = valid_tag_or(
        network::get_latest_github_release("junegunn/fzf", is_cn, http)
            .ok()
            .flatten(),
        FZF_FALLBACK_TAG,
    );
    let ver = tag.trim_start_matches('v');
    let url = format!(
        "https://github.com/junegunn/fzf/releases/download/{tag}/fzf-{ver}-linux_{arch_name}.tar.gz"
    );

    let tmp_dir = TempDir::new()?;
    let archive = tmp_dir.path().join(format!("fzf_{ver}_{arch_name}.tar.gz"));
    let extract_dir = tmp_dir.path().join("extract");

    if network::download_with_fallback(&archive, &url, is_cn, http).is_ok()
        && shared::extract_tar_gz(&archive, &extract_dir, runner).is_ok()
        && let Some(bin_fzf) = find_file_named(&extract_dir, "fzf")
    {
        install_binary_atomic(&bin_fzf, &ctx.path("/usr/local/bin/fzf"))?;
        return Ok(true);
    }

    if !ctx.is_cmd_present("fzf", runner) {
        warnings.push("fzf 安装失败，Fish / Micro / Yazi 模糊搜索可能不可用".to_string());
    }
    Ok(false)
}

/// Ensure `fd` is installed (via APT `fd-find` symlink or GitHub `sharkdp/fd` fallback).
pub fn ensure_fd(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("fd", runner) {
        return Ok(false);
    }

    if !ctx.no_sys && apt_pkg_available("fd-find", runner) {
        let _ = sys::safe_apt_install(&["fd-find"], runner);
    }
    if ctx.is_cmd_present("fdfind", runner) {
        let target = resolve_cmd_path(ctx, "fdfind");
        replace_symlink(&target, &ctx.path("/usr/local/bin/fd"))?;
        return Ok(true);
    }

    let arch = ctx.arch();
    let Some(target) = fd_target_triple(&arch) else {
        warnings.push(format!("fd: 不支持的架构 {arch}"));
        return Ok(false);
    };

    let is_cn = ctx.effective_is_cn();
    let tag = valid_tag_or(
        network::get_latest_github_release("sharkdp/fd", is_cn, http)
            .ok()
            .flatten(),
        FD_FALLBACK_TAG,
    );
    let url =
        format!("https://github.com/sharkdp/fd/releases/download/{tag}/fd-{tag}-{target}.tar.gz");

    let tmp_dir = TempDir::new()?;
    let archive = tmp_dir.path().join("fd.tar.gz");
    let extract_dir = tmp_dir.path().join("extract");

    if network::download_with_fallback(&archive, &url, is_cn, http).is_ok()
        && shared::extract_tar_gz(&archive, &extract_dir, runner).is_ok()
        && let Some(bin_fd) = find_file_named(&extract_dir, "fd")
    {
        install_binary_atomic(&bin_fd, &ctx.path("/usr/local/bin/fd"))?;
        return Ok(true);
    }

    if !ctx.is_cmd_present("fd", runner) {
        warnings.push("fd 安装失败，Yazi 文件搜索可能不可用".to_string());
    }
    Ok(false)
}

/// Ensure `ripgrep` (`rg`) is installed (APT or GitHub `.deb` fallback).
pub fn ensure_ripgrep(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("rg", runner) {
        return Ok(false);
    }

    if !ctx.no_sys && apt_pkg_available("ripgrep", runner) {
        let _ = sys::safe_apt_install(&["ripgrep"], runner);
        if ctx.is_cmd_present("rg", runner) {
            return Ok(true);
        }
    }

    let arch = ctx.arch();
    let Some(dpkg_arch) = github_dpkg_arch(&arch) else {
        warnings.push(format!("ripgrep: 不支持的架构 {arch}"));
        return Ok(false);
    };

    let is_cn = ctx.effective_is_cn();
    let tag = valid_tag_or(
        network::get_latest_github_release("BurntSushi/ripgrep", is_cn, http)
            .ok()
            .flatten(),
        RIPGREP_FALLBACK_TAG,
    );
    let ver = tag.trim_start_matches('v');
    let url = format!(
        "https://github.com/BurntSushi/ripgrep/releases/download/{tag}/ripgrep_{ver}-1_{dpkg_arch}.deb"
    );

    let tmp_dir = TempDir::new()?;
    let deb_path = tmp_dir
        .path()
        .join(format!("ripgrep_{ver}_{dpkg_arch}.deb"));
    if network::download_with_fallback(&deb_path, &url, is_cn, http).is_ok() && !ctx.no_sys {
        let deb_str = deb_path.to_string_lossy().into_owned();
        let dpkg_ok = runner
            .run("dpkg", &["-i", &deb_str], Duration::from_secs(120))
            .map(|o| o.success())
            .unwrap_or(false);
        if !dpkg_ok {
            let _ = runner.run_with_env(
                "apt-get",
                &["install", "-f", "-yq"],
                APT_ENV,
                Duration::from_secs(180),
            );
        }
        return Ok(true);
    }

    if !ctx.is_cmd_present("rg", runner) {
        warnings.push("ripgrep 安装失败，Micro / Yazi 内容搜索可能不可用".to_string());
    }
    Ok(false)
}

/// Ensure `zoxide` is installed (APT or upstream install script fallback).
pub fn ensure_zoxide(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("zoxide", runner) {
        return Ok(false);
    }

    if !ctx.no_sys && apt_pkg_available("zoxide", runner) {
        let _ = sys::safe_apt_install(&["zoxide"], runner);
        if ctx.is_cmd_present("zoxide", runner) {
            return Ok(true);
        }
    }

    let tmp_dir = TempDir::new()?;
    let script_path = tmp_dir.path().join("zoxide_install.sh");
    if network::download_with_fallback(
        &script_path,
        "https://raw.githubusercontent.com/ajeetdsouza/zoxide/main/install.sh",
        ctx.effective_is_cn(),
        http,
    )
    .is_ok()
        && !ctx.no_sys
    {
        let script_str = script_path.to_string_lossy().into_owned();
        let _ = runner.run("sh", &[&script_str, "-y"], Duration::from_secs(120));
        return Ok(true);
    }

    if !ctx.is_cmd_present("zoxide", runner) {
        warnings.push("zoxide 安装失败，Fish / Yazi 历史目录导航可能不可用".to_string());
    }
    Ok(false)
}

/// Ensure at least one clipboard tool (`xclip` / `wl-clipboard` / `xsel`) is installed.
pub fn ensure_clipboard(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    let has_any = |r: &dyn CommandRunner| {
        ctx.is_cmd_present("xclip", r)
            || ctx.is_cmd_present("wl-copy", r)
            || ctx.is_cmd_present("xsel", r)
    };
    if has_any(runner) {
        return Ok(false);
    }

    let mut installed = false;
    if !ctx.no_sys {
        if apt_pkg_available("xclip", runner) && sys::safe_apt_install(&["xclip"], runner).is_ok() {
            installed = true;
        }
        if !ctx.is_cmd_present("xclip", runner)
            && apt_pkg_available("wl-clipboard", runner)
            && sys::safe_apt_install(&["wl-clipboard"], runner).is_ok()
        {
            installed = true;
        }
        if !has_any(runner)
            && apt_pkg_available("xsel", runner)
            && sys::safe_apt_install(&["xsel"], runner).is_ok()
        {
            installed = true;
        }
    }

    if !installed && !has_any(runner) {
        warnings.push(
            "剪贴板工具（xclip / wl-clipboard / xsel）均未安装，终端剪贴板共享可能不可用"
                .to_string(),
        );
    }
    Ok(installed)
}

/// Ensure `bat` is installed (creating `/usr/local/bin/bat -> batcat` symlink on Debian).
pub fn ensure_bat(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("bat", runner) {
        return Ok(false);
    }

    if !ctx.no_sys && apt_pkg_available("bat", runner) {
        let _ = sys::safe_apt_install(&["bat"], runner);
    }
    if ctx.is_cmd_present("batcat", runner) {
        let target = resolve_cmd_path(ctx, "batcat");
        replace_symlink(&target, &ctx.path("/usr/local/bin/bat"))?;
        return Ok(true);
    }

    if !ctx.is_cmd_present("bat", runner) {
        warnings.push("bat 安装失败，MicroOmni 语法高亮预览可能不可用".to_string());
    }
    Ok(false)
}

fn fc_list_has_0xproto(runner: &dyn CommandRunner) -> bool {
    runner
        .run("fc-list", &[], Duration::from_secs(15))
        .ok()
        .filter(|o| o.success())
        .is_some_and(|o| o.stdout.to_ascii_lowercase().contains("0xprotonerdfont"))
}

/// Ensure `0xProto Nerd Font` is installed with `.debopti-installed` marker (`.tar.xz` -> `.zip` fallback).
pub fn ensure_0xproto_nerd_font(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if !ctx.no_sys {
        let _ = sys::safe_apt_install(&["fontconfig"], runner);
    }

    let font_dir = ctx.path("/usr/local/share/fonts/0xProto-Nerd-Font");
    let marker = font_dir.join(".debopti-installed");
    if marker.is_file() && fc_list_has_0xproto(runner) {
        return Ok(false);
    }

    if !ctx.no_sys {
        let mut installed_via_apt = false;
        for pkg in [
            "fonts-0xproto-nerd-font",
            "fonts-0xproto-nerd-font-mono",
            "fonts-0xproto-nerd-font-propo",
        ] {
            if apt_pkg_available(pkg, runner) && sys::safe_apt_install(&[pkg], runner).is_ok() {
                installed_via_apt = true;
            }
        }
        if installed_via_apt && fc_list_has_0xproto(runner) {
            fs::create_dir_all(&font_dir)?;
            sys::write_file_atomic(&marker, b"", Some(0o644))?;
            let _ = runner.run("fc-cache", &["-f"], Duration::from_secs(60));
            return Ok(true);
        }

        for pkg in ["xz-utils", "unzip"] {
            if apt_pkg_available(pkg, runner) {
                let _ = sys::safe_apt_install(&[pkg], runner);
            }
        }
    }

    let is_cn = ctx.effective_is_cn();
    let tag = valid_tag_or(
        network::get_latest_github_release("ryanoasis/nerd-fonts", is_cn, http)
            .ok()
            .flatten(),
        NERD_FONT_FALLBACK_TAG,
    );

    let tmp_dir = TempDir::new()?;
    let extract_dir = tmp_dir.path().join("extract");
    fs::create_dir_all(&extract_dir)?;
    fs::create_dir_all(&font_dir)?;

    let mut extracted = false;
    if ctx.is_cmd_present("xz", runner) {
        let tmp_xz = tmp_dir.path().join("0xProto.tar.xz");
        let url_xz = format!(
            "https://github.com/ryanoasis/nerd-fonts/releases/download/{tag}/0xProto.tar.xz"
        );
        if network::download_with_fallback(&tmp_xz, &url_xz, is_cn, http).is_ok() {
            let xz_str = tmp_xz.to_string_lossy().into_owned();
            let ex_str = extract_dir.to_string_lossy().into_owned();
            if runner
                .run(
                    "tar",
                    &["-xJf", &xz_str, "-C", &ex_str],
                    Duration::from_secs(120),
                )
                .map(|o| o.success())
                .unwrap_or(false)
            {
                extracted = true;
            }
        }
    }

    if !extracted && ctx.is_cmd_present("unzip", runner) {
        let _ = fs::remove_dir_all(&extract_dir);
        fs::create_dir_all(&extract_dir)?;
        let tmp_zip = tmp_dir.path().join("0xProto.zip");
        let url_zip =
            format!("https://github.com/ryanoasis/nerd-fonts/releases/download/{tag}/0xProto.zip");
        if network::download_with_fallback(&tmp_zip, &url_zip, is_cn, http).is_ok()
            && shared::extract_zip(&tmp_zip, &extract_dir, runner).is_ok()
        {
            extracted = true;
        }
    }

    if extracted {
        let mut copied = 0usize;
        for entry in WalkDir::new(&extract_dir)
            .into_iter()
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) else {
                continue;
            };
            if (ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"))
                && let Some(name) = entry.path().file_name()
            {
                let dest = font_dir.join(name);
                if fs::copy(entry.path(), &dest).is_ok() {
                    let _ = fs::set_permissions(&dest, fs::Permissions::from_mode(0o644));
                    copied += 1;
                }
            }
        }
        if copied > 0 {
            sys::write_file_atomic(&marker, b"", Some(0o644))?;
            if !ctx.no_sys {
                let dir_str = font_dir.to_string_lossy().into_owned();
                let ok = runner
                    .run("fc-cache", &["-f", &dir_str], Duration::from_secs(60))
                    .map(|o| o.success())
                    .unwrap_or(false);
                if !ok {
                    let _ = runner.run("fc-cache", &["-f"], Duration::from_secs(60));
                }
            }
            return Ok(true);
        }
        warnings.push("0xProto Nerd Font 解压后未找到字体文件".to_string());
    } else {
        warnings.push(
            "0xProto Nerd Font 下载或解压失败（需 xz-utils 或 unzip）；请在 SSH 客户端选用 Nerd Font"
                .to_string(),
        );
    }

    Ok(false)
}

/// Ensure `resvg` is installed (APT or x86_64 GitHub release fallback `v0.47.0`).
pub fn ensure_resvg(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
    warnings: &mut Vec<String>,
) -> Result<bool> {
    if ctx.is_cmd_present("resvg", runner) {
        return Ok(false);
    }

    if !ctx.no_sys && apt_pkg_available("resvg", runner) {
        let _ = sys::safe_apt_install(&["resvg"], runner);
        if ctx.is_cmd_present("resvg", runner) {
            return Ok(true);
        }
    }

    let arch = ctx.arch();
    if arch != "x86_64" {
        warnings.push(format!(
            "resvg: 当前架构 {arch} 无官方预编译包且 apt 源不可用，SVG 预览可能不可用"
        ));
        return Ok(false);
    }

    let is_cn = ctx.effective_is_cn();
    let tag = valid_tag_or(
        network::get_latest_github_release("linebender/resvg", is_cn, http)
            .ok()
            .flatten(),
        RESVG_FALLBACK_TAG,
    );
    let url = format!(
        "https://github.com/linebender/resvg/releases/download/{tag}/resvg-linux-x86_64.tar.gz"
    );

    let tmp_dir = TempDir::new()?;
    let archive = tmp_dir.path().join("resvg.tar.gz");
    let extract_dir = tmp_dir.path().join("extract");

    if network::download_with_fallback(&archive, &url, is_cn, http).is_ok()
        && shared::extract_tar_gz(&archive, &extract_dir, runner).is_ok()
        && let Some(bin_resvg) = find_file_named(&extract_dir, "resvg")
    {
        install_binary_atomic(&bin_resvg, &ctx.path("/usr/local/bin/resvg"))?;
        return Ok(true);
    }

    if !ctx.is_cmd_present("resvg", runner) {
        warnings.push("resvg 安装失败，SVG 预览可能不可用".to_string());
    }
    Ok(false)
}

/// Unified idempotent installer for shared Fish / Micro / Yazi CLI dependencies.
/// Returns any non-fatal warnings emitted during best-effort installation.
pub fn install_devops_shared_cli_deps(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<Vec<String>> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "ensuring shared DevOps CLI dependencies",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run_with_env(
            "apt-get",
            &["update", "-yq"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    let mut warnings = Vec::new();
    let _ = ensure_fzf(ctx, http, runner, &mut warnings)?;
    let _ = ensure_fd(ctx, http, runner, &mut warnings)?;
    let _ = ensure_ripgrep(ctx, http, runner, &mut warnings)?;
    let _ = ensure_zoxide(ctx, http, runner, &mut warnings)?;
    let _ = ensure_clipboard(ctx, runner, &mut warnings)?;
    let _ = ensure_bat(ctx, runner, &mut warnings)?;
    let _ = ensure_0xproto_nerd_font(ctx, http, runner, &mut warnings)?;
    Ok(warnings)
}
