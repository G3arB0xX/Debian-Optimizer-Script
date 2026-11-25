use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use walkdir::WalkDir;

use crate::agent_workspace;
use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};
use crate::version;

pub const MODULE_ID: &str = "self-manage";
pub const DEFAULT_REPO: &str = "G3arB0xX/Debian-Optimizer-Script";

const DIAG_TARGET: &str = "debopty::apps::self_manage";

const DEV_STRIP_ENTRIES: &[&str] = &[
    ".git",
    ".jj",
    ".github",
    ".jj_backup",
    ".cursor",
    ".agents",
    "tools",
    "governance",
    "instructions",
    "tests",
    "rust",
    "archive",
    "AGENTS.md",
    "CLAUDE.md",
    "GEMINI.md",
    "Makefile",
    ".shellcheckrc",
    ".editorconfig",
    ".gitignore",
];

fn app_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: MODULE_ID.to_string(),
        message: message.into(),
    }
}

/// Options for `self_install`.
#[derive(Debug, Clone, Default)]
pub struct SelfInstallOptions {
    pub source_bin: Option<PathBuf>,
    pub install_dir: Option<PathBuf>,
    pub bin_link: Option<PathBuf>,
    pub f2b_bin_link: Option<PathBuf>,
    pub rs_bin_link: Option<PathBuf>,
    pub skip_agent_workspace: bool,
    pub agent_workspace_dir: Option<PathBuf>,
    pub skip_default_tools: bool,
    pub skip_retention: bool,
}

/// Options for `self_update`.
#[derive(Debug, Clone, Default)]
pub struct SelfUpdateOptions {
    pub repo: Option<String>,
    pub release_base_url: Option<String>,
    pub tag: Option<String>,
    pub force: bool,
    pub install_dir: Option<PathBuf>,
}

/// Options for `self_uninstall`.
#[derive(Debug, Clone)]
pub struct SelfUninstallOptions {
    pub keep_config: bool,
    pub install_dir: Option<PathBuf>,
}

impl Default for SelfUninstallOptions {
    fn default() -> Self {
        Self {
            keep_config: true,
            install_dir: None,
        }
    }
}

/// Pure-Rust FIPS 180-4 SHA-256 digest returning lowercase 64-character hex.
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity((data.len() + 9).div_ceil(64) * 64);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w[..16].iter_mut().enumerate() {
            let off = i * 4;
            *word =
                u32::from_be_bytes([chunk[off], chunk[off + 1], chunk[off + 2], chunk[off + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

/// Computes lowercase SHA-256 hex digest of a file on disk.
pub fn sha256_file_hex(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(sha256_hex(&bytes))
}

fn find_sha256sums_expected(sums_text: &str, asset_name: &str) -> Option<String> {
    if asset_name.trim().is_empty() {
        return None;
    }

    for raw_line in sums_text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let mut split = line.splitn(2, |c: char| c.is_ascii_whitespace());
        let Some(hash) = split.next() else {
            continue;
        };
        let Some(rest) = split.next() else {
            continue;
        };
        let fname = rest.trim_start();
        let fname = fname.strip_prefix('*').unwrap_or(fname);
        let fname = fname.strip_prefix("./").unwrap_or(fname);

        if fname == asset_name && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn has_sha256sums_entry(sums_text: &str, asset_name: &str) -> bool {
    find_sha256sums_expected(sums_text, asset_name).is_some()
}

/// Verifies `file_path` against a `SHA256SUMS` manifest for `asset_name`.
pub fn verify_sha256sums(file_path: &Path, sums_text: &str, asset_name: &str) -> Result<bool> {
    if asset_name.trim().is_empty() || !file_path.is_file() {
        return Ok(false);
    }

    let Some(expected_hex) = find_sha256sums_expected(sums_text, asset_name) else {
        return Ok(false);
    };
    let actual_hex = sha256_file_hex(file_path)?;
    Ok(actual_hex == expected_hex)
}

fn replace_symlink(target: &Path, link_path: &Path) -> Result<()> {
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::symlink_metadata(link_path).is_ok() {
        let _ = fs::remove_file(link_path);
    }
    symlink(target, link_path)?;
    Ok(())
}

fn strip_dev_metadata(install_dir: &Path) {
    for entry in DEV_STRIP_ENTRIES {
        let target = install_dir.join(entry);
        if let Ok(meta) = fs::symlink_metadata(&target) {
            if meta.is_dir() && !meta.file_type().is_symlink() {
                let _ = fs::remove_dir_all(&target);
            } else {
                let _ = fs::remove_file(&target);
            }
        }
    }

    for entry in WalkDir::new(install_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file() && entry.file_name().to_str() == Some("INDEX.md") {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn map_release_arch(raw_arch: &str) -> Result<&'static str> {
    match raw_arch.trim() {
        "x86_64" | "amd64" => Ok("amd64"),
        "aarch64" | "arm64" => Ok("arm64"),
        other => Err(app_err(format!("unsupported architecture: '{other}'"))),
    }
}

/// Installs the native Rust `debopty` binary under `/opt/debopty/bin/debopty`, wires
/// primary/multicall/compat symlinks, migrates legacy `debopti` shortcuts, renders shell
/// profiles, strips dev/archive metadata, and persists version identity.
pub fn self_install(
    ctx: &AppContext,
    opts: &SelfInstallOptions,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let install_dir = opts
        .install_dir
        .clone()
        .unwrap_or_else(|| ctx.path("/opt/debopty"));
    let bin_dir = install_dir.join("bin");
    let dest_bin = bin_dir.join("debopty");
    fs::create_dir_all(&bin_dir)?;

    let source_bin = match &opts.source_bin {
        Some(p) => p.clone(),
        None => env::current_exe()?,
    };
    if !source_bin.is_file() {
        return Err(app_err(format!(
            "source binary not found: {}",
            source_bin.display()
        )));
    }

    let same_file = match (fs::canonicalize(&source_bin), fs::canonicalize(&dest_bin)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if !same_file {
        let bytes = fs::read(&source_bin)?;
        sys::write_file_atomic(&dest_bin, &bytes, Some(0o755))?;
    } else {
        fs::set_permissions(&dest_bin, fs::Permissions::from_mode(0o755))?;
    }

    strip_dev_metadata(&install_dir);

    let bin_link = opts
        .bin_link
        .clone()
        .unwrap_or_else(|| ctx.path("/usr/local/bin/debopty"));
    let f2b_link = opts
        .f2b_bin_link
        .clone()
        .unwrap_or_else(|| ctx.path("/usr/local/bin/debopty-f2b-sync"));
    let rs_link = opts
        .rs_bin_link
        .clone()
        .unwrap_or_else(|| ctx.path("/usr/local/bin/debopty-rs"));

    replace_symlink(&dest_bin, &bin_link)?;
    replace_symlink(&dest_bin, &f2b_link)?;
    replace_symlink(&dest_bin, &rs_link)?;

    // Migrate/remove legacy `debopti` shortcut, install dir, Fish PATH, and shell profile artifacts.
    let legacy_bin = ctx.path("/usr/local/bin/debopti");
    if let Ok(meta) = fs::symlink_metadata(&legacy_bin) {
        if meta.file_type().is_symlink() {
            let should_remove = match fs::read_link(&legacy_bin) {
                Ok(target) => {
                    let s = target.to_string_lossy();
                    s.contains("deb_optimizer.sh")
                        || s.contains("debopti")
                        || s.contains("debopty")
                        || !legacy_bin.exists()
                }
                Err(_) => true,
            };
            if should_remove {
                let _ = fs::remove_file(&legacy_bin);
            }
        } else if meta.file_type().is_file()
            && let Ok(file) = fs::File::open(&legacy_bin)
        {
            let mut buf = Vec::with_capacity(4096);
            if file.take(4096).read_to_end(&mut buf).is_ok() {
                let content = String::from_utf8_lossy(&buf);
                if content.contains("deb_optimizer.sh")
                    || content.contains("/opt/debopti")
                    || content.contains("/opt/debopty")
                {
                    let _ = fs::remove_file(&legacy_bin);
                }
            }
        }
    }

    let mut migrated_legacy_dir: Option<String> = None;
    let legacy_opt = ctx.path("/opt/debopti");
    if legacy_opt != install_dir
        && let Ok(meta) = fs::symlink_metadata(&legacy_opt)
    {
        if meta.file_type().is_symlink() {
            let _ = fs::remove_file(&legacy_opt);
        } else if legacy_opt.is_dir()
            && (legacy_opt.join("deb_optimizer.sh").is_file()
                || legacy_opt.join("VERSION").is_file()
                || legacy_opt.join("scripts/common.sh").is_file())
            && fs::remove_dir_all(&legacy_opt).is_ok()
        {
            migrated_legacy_dir = Some(legacy_opt.to_string_lossy().into_owned());
        }
    }

    let _ = shared::remove_fish_path(ctx, "/opt/debopti", runner);
    let _ = fs::remove_file(ctx.path("/etc/profile.d/debopti.sh"));
    let _ = fs::remove_file(ctx.path("/etc/fish/conf.d/debopti.fish"));

    let tpl_engine = ctx.template_engine();
    let mut vars = BTreeMap::new();
    let bin_link_value = if ctx.is_custom_root() {
        bin_link.to_string_lossy().into_owned()
    } else {
        "/usr/local/bin/debopty".to_string()
    };
    vars.insert("BIN_LINK".to_string(), bin_link_value);

    let profile_sh = ctx.path("/etc/profile.d/debopty.sh");
    tpl_engine.render_to_file(
        "templates/common/debopty.sh",
        &profile_sh,
        &vars,
        Some(0o755),
    )?;

    let fish_conf_dir = ctx.path("/etc/fish/conf.d");
    if ctx.is_cmd_present("fish", runner) || fish_conf_dir.is_dir() {
        let profile_fish = fish_conf_dir.join("debopty.fish");
        tpl_engine.render_to_file(
            "templates/common/debopty.fish",
            &profile_fish,
            &vars,
            Some(0o644),
        )?;
    }

    let id = version::compiled_identity();
    let store = ctx.config_store();
    store.set("INSTALLED", "true")?;
    if let Some(primary) = id.primary() {
        store.set("SCRIPT_VERSION", primary)?;
    }
    store.set("SCRIPT_BUILD", &id.build)?;

    let mut agent_ws_path: Option<String> = None;
    if !opts.skip_agent_workspace
        && env::var("DEBOPTY_SKIP_AGENT_WORKSPACE").ok().as_deref() != Some("1")
    {
        let ws_dir = opts
            .agent_workspace_dir
            .clone()
            .unwrap_or_else(|| ctx.path(agent_workspace::DEFAULT_WORKSPACE_DIR));
        let mut ws_opts = agent_workspace::AgentWorkspaceOptions::new(&ctx.root_prefix, ws_dir);
        ws_opts.template_dir = ctx.template_dir.clone();
        let ws_report = agent_workspace::init_workspace(&ws_opts)?;
        agent_ws_path = Some(ws_report.workspace_dir);
    }

    let mut retention_status: Option<String> = None;
    let mut retention_warning: Option<String> = None;
    if !opts.skip_retention
        && !ctx.no_sys
        && env::var("DEBOPTY_SKIP_RETENTION").ok().as_deref() != Some("1")
    {
        let mut sys_ctx = crate::system::SystemContext::with_root(&ctx.root_prefix);
        sys_ctx.template_dir = ctx.template_dir.clone();
        sys_ctx.config_path = ctx.config_path.clone();
        sys_ctx.no_sys = ctx.no_sys;
        match crate::system::setup_retention(
            &sys_ctx,
            &crate::system::RetentionOptions::default(),
            runner,
        ) {
            Ok(_) => retention_status = Some("configured".to_string()),
            Err(err) => {
                retention_warning = Some(format!("声明式日志与冷缓存留存配置跳过: {err}"));
            }
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "self-install completed",
        None,
        &[
            ("binary", &dest_bin.to_string_lossy()),
            ("bin_link", &bin_link.to_string_lossy()),
            ("f2b_link", &f2b_link.to_string_lossy()),
        ],
    );

    let mut report = AppReport::new(
        MODULE_ID,
        "self-install",
        true,
        format!("debopty 原生二进制已安装至 {}", dest_bin.display()),
    )
    .with_detail("binary", dest_bin.to_string_lossy().into_owned())
    .with_detail("bin_link", bin_link.to_string_lossy().into_owned())
    .with_detail("f2b_bin_link", f2b_link.to_string_lossy().into_owned())
    .with_detail("rs_bin_link", rs_link.to_string_lossy().into_owned());
    if let Some(dir) = migrated_legacy_dir {
        report = report.with_detail("migrated_legacy_dir", dir);
    }
    if let Some(ws) = agent_ws_path {
        report = report.with_detail("agent_workspace", ws);
    }
    if let Some(st) = retention_status {
        report = report.with_detail("retention", st);
    }
    if let Some(warn) = retention_warning {
        report = report.with_warning(warn);
    }
    Ok(report)
}

/// Best-effort idempotent installer for default companion tools (`tmux` + `sendme`).
pub fn ensure_default_tools(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> AppReport {
    if ctx.no_sys || env::var("DEBOPTY_SKIP_DEFAULT_TOOLS").ok().as_deref() == Some("1") {
        return AppReport::new(
            MODULE_ID,
            "default-tools",
            false,
            "已跳过默认配套工具安装 (sendme/tmux)",
        )
        .with_detail("skipped", "true");
    }

    let mut installed = Vec::new();
    let mut warnings = Vec::new();

    let mut sys_ctx = crate::system::SystemContext::with_root(&ctx.root_prefix);
    sys_ctx.template_dir = ctx.template_dir.clone();
    sys_ctx.no_sys = ctx.no_sys;
    if let Err(err) = crate::system::setup_tmux(&sys_ctx, runner) {
        warnings.push(format!("tmux 安装/配置失败: {err}"));
    } else {
        installed.push("tmux");
    }

    match crate::apps::devops::ensure_sendme(ctx, http, runner, &mut warnings) {
        Ok(true) => installed.push("sendme"),
        Ok(false) => {
            if ctx.is_cmd_present("sendme", runner) {
                installed.push("sendme");
            }
        }
        Err(err) => warnings.push(format!("sendme 安装失败: {err}")),
    }

    let summary = if warnings.is_empty() {
        format!("默认配套工具已就绪: {}", installed.join(", "))
    } else {
        format!(
            "默认配套工具部分就绪 ({})，警告: {}",
            installed.join(", "),
            warnings.join("; ")
        )
    };
    let mut report = AppReport::new(MODULE_ID, "default-tools", true, summary);
    if !installed.is_empty() {
        report = report.with_detail("default_tools", installed.join(","));
    }
    if !warnings.is_empty() {
        report = report.with_detail("default_tools_warnings", warnings.join("; "));
    }
    report
}

/// Installs `debopty` and best-effort ensures default companion tools (`sendme` and `tmux`).
pub fn self_install_with_http(
    ctx: &AppContext,
    opts: &SelfInstallOptions,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let mut report = self_install(ctx, opts, runner)?;
    if !opts.skip_default_tools
        && !ctx.no_sys
        && env::var("DEBOPTY_SKIP_DEFAULT_TOOLS").ok().as_deref() != Some("1")
    {
        let tools_report = ensure_default_tools(ctx, http, runner);
        if let Some(tools) = tools_report.details.get("default_tools") {
            report = report.with_detail("default_tools", tools.clone());
        }
        if let Some(warn) = tools_report.details.get("default_tools_warnings") {
            report = report.with_detail("default_tools_warnings", warn.clone());
        }
    }
    Ok(report)
}

/// Downloads, SHA-256 verifies, and atomically upgrades `/opt/debopty/bin/debopty`.
pub fn self_update(
    ctx: &AppContext,
    opts: &SelfUpdateOptions,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let arch = map_release_arch(&ctx.arch())?;
    let repo = opts
        .repo
        .clone()
        .or_else(|| env::var("DEBOPTY_REPO").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| DEFAULT_REPO.to_string());
    let is_cn = ctx.effective_is_cn();

    let raw_tag = match &opts.tag {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => match network::get_latest_github_release(&repo, is_cn, http)? {
            Some(latest) if !latest.trim().is_empty() => latest.trim().to_string(),
            _ => return Err(app_err("无法获取最新 GitHub Release 标签")),
        },
    };
    let tag = if raw_tag.starts_with('v') {
        raw_tag
    } else {
        format!("v{raw_tag}")
    };
    let target_ver = tag.trim_start_matches('v');

    let current_id = version::compiled_identity();
    if !opts.force && current_id.semver.as_deref() == Some(target_ver) {
        return Ok(AppReport::new(
            MODULE_ID,
            "self-update",
            false,
            format!(
                "当前已是最新版本 {}，无需更新",
                version::format_display(&current_id)
            ),
        )
        .with_detail("tag", &tag));
    }

    let asset_name = format!("debopty-{tag}-linux-{arch}.tar.gz");
    let base_url = match &opts.release_base_url {
        Some(base) if !base.trim().is_empty() => base.trim_end_matches('/').to_string(),
        _ => format!("https://github.com/{repo}/releases/download/{tag}"),
    };

    let tmp_dir = TempDir::new()?;
    let sums_path = tmp_dir.path().join("SHA256SUMS");
    let archive_path = tmp_dir.path().join(&asset_name);

    network::download_with_fallback_expect(
        &sums_path,
        &format!("{base_url}/SHA256SUMS"),
        is_cn,
        network::DownloadExpect::Sha256Sums,
        http,
    )
    .map_err(|e| app_err(format!("下载 SHA256SUMS 失败: {e}")))?;

    let sums_text = fs::read_to_string(&sums_path)?;
    if !has_sha256sums_entry(&sums_text, &asset_name) {
        return Err(app_err(format!(
            "SHA256SUMS 中缺少目标架构资产 {asset_name} (tag={tag}, arch={arch})"
        )));
    }

    let mut saw_sha_mismatch = false;
    if let Err(e) = network::download_with_fallback_validated(
        &archive_path,
        &format!("{base_url}/{asset_name}"),
        is_cn,
        http,
        |cand| {
            if network::looks_like_html_file(cand) {
                return Err("soft-200 HTML error page".to_string());
            }
            if verify_sha256sums(cand, &sums_text, &asset_name).unwrap_or(false) {
                Ok(())
            } else {
                saw_sha_mismatch = true;
                Err("SHA-256 mismatch".to_string())
            }
        },
    ) {
        if saw_sha_mismatch {
            return Err(app_err(format!(
                "SHA-256 校验失败，已拒绝安装被篡改或不完整的资产 {asset_name} ({e})"
            )));
        }
        return Err(app_err(format!("下载发布包 {asset_name} 失败: {e}")));
    }

    if !verify_sha256sums(&archive_path, &sums_text, &asset_name)? {
        return Err(app_err(format!(
            "SHA-256 校验失败，已拒绝安装被篡改或不完整的资产 {asset_name}"
        )));
    }

    let extract_dir = tmp_dir.path().join("extract");
    fs::create_dir_all(&extract_dir)?;
    shared::extract_tar_gz(&archive_path, &extract_dir, runner)
        .map_err(|e| app_err(format!("解压发布包失败: {e}")))?;

    let extracted_bin = extract_dir.join("debopty");
    if !extracted_bin.is_file() {
        return Err(app_err(format!(
            "发布包 {asset_name} 中缺少 debopty 可执行文件"
        )));
    }

    let install_opts = SelfInstallOptions {
        source_bin: Some(extracted_bin),
        install_dir: opts.install_dir.clone(),
        ..SelfInstallOptions::default()
    };
    let mut report = self_install(ctx, &install_opts, runner)?;
    report.action = "self-update".to_string();
    report.summary = format!("debopty 已原子升级至 {tag} ({arch})");
    report.details.insert("tag".to_string(), tag.clone());
    report.details.insert("arch".to_string(), arch.to_string());

    // BE-P1-16: resolve the NEW release's build ID (from archive VERSION or newly installed binary)
    // instead of keeping the old running binary's compiled_identity().build.
    let mut resolved_build: Option<String> = None;
    let extracted_version = extract_dir.join("VERSION");
    if extracted_version.is_file() {
        if let Ok(parsed_id) = version::parse_version_file(&extracted_version) {
            if !parsed_id.build.is_empty() {
                resolved_build = Some(parsed_id.build.clone());
            }
        } else if let Ok(text) = fs::read_to_string(&extracted_version) {
            for line in text.lines() {
                if let Some(rest) = line.trim().strip_prefix("DEBOPTI_BUILD=") {
                    let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
                    if !val.is_empty() {
                        resolved_build = Some(val.to_string());
                        break;
                    }
                }
            }
        }
        let install_dir = opts
            .install_dir
            .clone()
            .unwrap_or_else(|| ctx.path("/opt/debopty"));
        if let Ok(raw_ver) = fs::read(&extracted_version) {
            let _ = sys::write_file_atomic(&install_dir.join("VERSION"), &raw_ver, Some(0o644));
        }
    }
    if resolved_build.is_none()
        && !ctx.no_sys
        && let Some(new_bin) = report.details.get("binary")
        && let Ok(out) = runner.run(new_bin, &["version", "--json"], Duration::from_secs(10))
        && out.success()
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&out.stdout)
        && let Some(b) = val.get("build").and_then(serde_json::Value::as_str)
        && !b.trim().is_empty()
    {
        resolved_build = Some(b.trim().to_string());
    }
    let final_build = resolved_build.unwrap_or_else(|| format!("release-{target_ver}"));
    let _ = ctx.config_store().set("SCRIPT_VERSION", target_ver);
    let _ = ctx.config_store().set("SCRIPT_BUILD", &final_build);
    report
        .details
        .insert("script_build".to_string(), final_build);

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "self-update completed",
        None,
        &[("tag", &tag), ("arch", arch)],
    );

    Ok(report)
}

/// Uninstalls `debopty` binaries, shortcuts, and Fish environment hooks while respecting
/// `U8` preservation of `/etc/debopti` when `opts.keep_config` is `true`.
pub fn self_uninstall(
    ctx: &AppContext,
    opts: &SelfUninstallOptions,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    // BE-P1-16: stop, disable, and remove debopty-f2b-port-sync units before deleting /usr/local/bin/debopty-f2b-sync
    if !ctx.no_sys {
        for unit in [
            "debopty-f2b-port-sync.timer",
            "debopty-f2b-port-sync.path",
            "debopty-f2b-port-sync.service",
            "debopty-apt-clean.timer",
            "debopty-apt-clean.service",
        ] {
            let _ = runner.run(
                "systemctl",
                &["disable", "--now", unit],
                Duration::from_secs(15),
            );
        }
    }

    for rel in [
        "/etc/systemd/system/debopty-f2b-port-sync.service",
        "/etc/systemd/system/debopty-f2b-port-sync.path",
        "/etc/systemd/system/debopty-f2b-port-sync.timer",
        "/etc/systemd/system/debopty-apt-clean.service",
        "/etc/systemd/system/debopty-apt-clean.timer",
        "/etc/apt/apt.conf.d/99-debopty-apt-clean",
        "/usr/local/bin/debopty",
        "/usr/local/bin/debopty-rs",
        "/usr/local/bin/debopty-f2b-sync",
        "/usr/local/bin/debopti",
        "/etc/profile.d/debopty.sh",
        "/etc/profile.d/debopti.sh",
        "/etc/fish/conf.d/debopty.fish",
        "/etc/fish/conf.d/debopti.fish",
    ] {
        let p = ctx.path(rel);
        if fs::symlink_metadata(&p).is_ok() {
            let _ = fs::remove_file(&p);
        }
    }

    let _ = shared::remove_fish_path(ctx, "/opt/debopty", runner);
    let _ = shared::remove_fish_path(ctx, "/opt/debopti", runner);
    let _ = shared::remove_fish_env(ctx, "IS_CN_REGION", runner);

    let install_dir = opts
        .install_dir
        .clone()
        .unwrap_or_else(|| ctx.path("/opt/debopty"));
    if install_dir.exists() {
        let _ = fs::remove_dir_all(&install_dir);
    }
    let legacy_install_dir = ctx.path("/opt/debopti");
    if legacy_install_dir.exists() {
        let _ = fs::remove_dir_all(&legacy_install_dir);
    }

    let mut report = AppReport::new(
        MODULE_ID,
        "self-uninstall",
        true,
        "debopty 已卸载完成（不影响已生效的内核或防火墙规则）",
    );

    let conf_dir = ctx.path("/etc/debopti");
    let agent_ws_dir = ctx.path(agent_workspace::DEFAULT_WORKSPACE_DIR);
    if opts.keep_config {
        if conf_dir.exists() {
            report = report.with_preserved(format!("保留持久化配置目录 {}", conf_dir.display()));
        }
        if agent_ws_dir.exists() {
            report =
                report.with_preserved(format!("保留 Agent 预设工作区 {}", agent_ws_dir.display()));
        }
    } else {
        if conf_dir.exists() {
            let _ = fs::remove_dir_all(&conf_dir);
            report =
                report.with_detail("purged_config_dir", conf_dir.to_string_lossy().into_owned());
        }
        if agent_ws_dir.exists() {
            let _ = fs::remove_dir_all(&agent_ws_dir);
            report = report.with_detail(
                "purged_agent_workspace",
                agent_ws_dir.to_string_lossy().into_owned(),
            );
        }
        let journald_dropin = ctx.path("/etc/systemd/journald.conf.d/99-debopty-retention.conf");
        if journald_dropin.is_file() {
            let _ = fs::remove_file(&journald_dropin);
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "self-uninstall completed",
        None,
        &[(
            "keep_config",
            if opts.keep_config { "true" } else { "false" },
        )],
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::CmdOutput;

    struct NoopRunner;
    impl CommandRunner for NoopRunner {
        fn run(&self, _prog: &str, _args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
            Ok(CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }

    #[test]
    fn sha256_matches_nist_fips_180_4_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn verify_sha256sums_handles_variants_and_rejects_tampering() {
        let tmp = TempDir::new().expect("tempdir");
        let asset = tmp.path().join("debopty-v2.0.0-linux-amd64.tar.gz");
        fs::write(&asset, b"debopty-static-binary-tarball").expect("write asset");
        let digest = sha256_hex(b"debopty-static-binary-tarball");

        let sums_std = format!("{digest}  debopty-v2.0.0-linux-amd64.tar.gz\n");
        let sums_bin = format!(
            "{} *./debopty-v2.0.0-linux-amd64.tar.gz\n",
            digest.to_uppercase()
        );
        let sums_bad = "0000000000000000000000000000000000000000000000000000000000000000  debopty-v2.0.0-linux-amd64.tar.gz\n";

        assert!(verify_sha256sums(&asset, &sums_std, "debopty-v2.0.0-linux-amd64.tar.gz").unwrap());
        assert!(verify_sha256sums(&asset, &sums_bin, "debopty-v2.0.0-linux-amd64.tar.gz").unwrap());
        assert!(!verify_sha256sums(&asset, sums_bad, "debopty-v2.0.0-linux-amd64.tar.gz").unwrap());
        assert!(!verify_sha256sums(&asset, &sums_std, "other.tar.gz").unwrap());
    }

    #[test]
    fn self_install_and_uninstall_preserve_u8_config_and_strip_dev_dirs() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("rootfs");
        let fake_bin = tmp.path().join("debopty-bin");
        fs::write(&fake_bin, b"#!/bin/sh\necho debopty\n").expect("write fake bin");
        fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).expect("chmod");

        // Seed legacy shortcuts and dev/archive clutter inside /opt/debopty
        fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        fs::create_dir_all(root.join("etc/profile.d")).unwrap();
        fs::create_dir_all(root.join("etc/fish/conf.d")).unwrap();
        fs::create_dir_all(root.join("opt/debopty/archive/legacy-bash")).unwrap();
        fs::create_dir_all(root.join("opt/debopty/rust")).unwrap();
        fs::create_dir_all(root.join("opt/debopty/governance")).unwrap();
        fs::create_dir_all(root.join("opt/debopty/.agents")).unwrap();
        fs::write(root.join("opt/debopty/INDEX.md"), "index").unwrap();
        symlink(
            root.join("opt/debopti/deb_optimizer.sh"),
            root.join("usr/local/bin/debopti"),
        )
        .unwrap();
        fs::write(root.join("etc/profile.d/debopti.sh"), "alias debopti=old\n").unwrap();

        let mut ctx = AppContext::with_root(&root);
        ctx.no_sys = true;
        let runner = NoopRunner;

        let opts = SelfInstallOptions {
            source_bin: Some(fake_bin),
            ..SelfInstallOptions::default()
        };
        let report = self_install(&ctx, &opts, &runner).expect("self_install");
        assert!(report.changed);
        assert!(root.join("opt/debopty/bin/debopty").is_file());
        assert!(root.join("usr/local/bin/debopty").is_symlink());
        assert!(root.join("usr/local/bin/debopty-f2b-sync").is_symlink());
        assert!(root.join("usr/local/bin/debopty-rs").is_symlink());
        assert!(!root.join("usr/local/bin/debopti").exists());
        assert!(!root.join("etc/profile.d/debopti.sh").exists());
        assert!(root.join("etc/profile.d/debopty.sh").is_file());
        assert!(root.join("etc/fish/conf.d/debopty.fish").is_file());
        assert!(!root.join("opt/debopty/archive").exists());
        assert!(!root.join("opt/debopty/rust").exists());
        assert!(!root.join("opt/debopty/governance").exists());
        assert!(!root.join("opt/debopty/.agents").exists());
        assert!(!root.join("opt/debopty/INDEX.md").exists());

        // Auto-provisioned VPS agent workspace under /var/lib/debopty/agent
        let ws_root = root.join("var/lib/debopty/agent");
        assert!(report.details.contains_key("agent_workspace"));
        assert!(ws_root.join("AGENTS.md").is_file());
        assert!(ws_root.join(".agents/MEMORY.md").is_file());
        assert!(ws_root.join(".agents/workspace.json").is_file());

        // Default uninstall preserves /etc/debopti and /var/lib/debopty/agent (U8)
        let un_keep = self_uninstall(&ctx, &SelfUninstallOptions::default(), &runner)
            .expect("self_uninstall keep");
        assert!(un_keep.changed);
        assert!(!root.join("opt/debopty").exists());
        assert!(!root.join("usr/local/bin/debopty").exists());
        assert!(!root.join("usr/local/bin/debopty-f2b-sync").exists());
        assert!(root.join("etc/debopti/debopti.conf").is_file());
        assert!(ws_root.join(".agents/MEMORY.md").is_file());
        assert!(
            un_keep
                .preserved_user_custom
                .iter()
                .any(|s| s.contains("Agent 预设工作区"))
        );

        // Purge uninstall removes /etc/debopti and /var/lib/debopty/agent
        let un_purge = self_uninstall(
            &ctx,
            &SelfUninstallOptions {
                keep_config: false,
                install_dir: None,
            },
            &runner,
        )
        .expect("self_uninstall purge");
        assert!(un_purge.changed);
        assert!(!root.join("etc/debopti").exists());
        assert!(!ws_root.exists());
        assert!(un_purge.details.contains_key("purged_agent_workspace"));
    }

    struct TarStubRunner;
    impl CommandRunner for TarStubRunner {
        fn run(&self, prog: &str, args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
            if prog == "tar"
                && let Some(pos) = args.iter().position(|a| *a == "-C")
                && let Some(dest) = args.get(pos + 1)
            {
                let dest_dir = Path::new(dest);
                fs::create_dir_all(dest_dir)?;
                let bin = dest_dir.join("debopty");
                fs::write(&bin, b"#!/bin/sh\necho debopty-new\n")?;
                fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))?;
                fs::write(
                    dest_dir.join("VERSION"),
                    "DEBOPTI_SEMVER=\"9.9.9\"\nDEBOPTI_BUILD=\"release-9.9.9\"\n",
                )?;
            }
            Ok(CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            })
        }
    }

    #[test]
    fn self_update_skips_soft_200_html_cn_mirrors_and_recovers() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("rootfs");
        fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        fs::create_dir_all(root.join("etc/profile.d")).unwrap();
        fs::create_dir_all(root.join("etc/fish/conf.d")).unwrap();

        let mut ctx = AppContext::with_root(&root);
        ctx.no_sys = true;
        ctx.is_cn_region = true;
        ctx.arch_override = Some("x86_64".to_string());

        let tag = "v9.9.9";
        let asset_name = "debopty-v9.9.9-linux-amd64.tar.gz";
        let base_url = format!("https://github.com/{DEFAULT_REPO}/releases/download/{tag}");
        let sums_orig = format!("{base_url}/SHA256SUMS");
        let asset_orig = format!("{base_url}/{asset_name}");

        let archive_bytes = b"\x1f\x8b\x08\x00valid-release-tarball".to_vec();
        let digest = sha256_hex(&archive_bytes);
        let sums_body = format!("{digest}  {asset_name}\n").into_bytes();

        let http = network::MockHttpClient::new();
        // Mirror 1 (gh-proxy.com) returns soft-200 Cloudflare HTML for both SHA256SUMS and archive
        http.on_download(
            format!("https://gh-proxy.com/{sums_orig}"),
            200,
            b"<!DOCTYPE html><html><body>CF challenge</body></html>".to_vec(),
        );
        http.on_download(
            format!("https://gh-proxy.com/{asset_orig}"),
            200,
            b"<!DOCTYPE html><html><body>CF challenge</body></html>".to_vec(),
        );
        // Mirror 2 (cdn.gh-proxy.org) serves real SHA256SUMS and archive
        http.on_download(
            format!("https://cdn.gh-proxy.org/{sums_orig}"),
            200,
            sums_body,
        );
        http.on_download(
            format!("https://cdn.gh-proxy.org/{asset_orig}"),
            200,
            archive_bytes,
        );

        let opts = SelfUpdateOptions {
            tag: Some(tag.to_string()),
            force: true,
            ..SelfUpdateOptions::default()
        };
        let report = self_update(&ctx, &opts, &http, &TarStubRunner).expect("self_update");
        assert!(report.changed);
        assert_eq!(report.details.get("tag").map(String::as_str), Some(tag));
        assert!(root.join("opt/debopty/bin/debopty").is_file());
        assert_eq!(
            fs::read(root.join("opt/debopty/bin/debopty")).unwrap(),
            b"#!/bin/sh\necho debopty-new\n"
        );
    }

    #[test]
    fn self_update_rejects_sha256_mismatch_and_missing_asset_entry() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path().join("rootfs");
        let mut ctx = AppContext::with_root(&root);
        ctx.no_sys = true;
        ctx.is_cn_region = false;
        ctx.arch_override = Some("x86_64".to_string());

        let tag = "v9.9.9";
        let asset_name = "debopty-v9.9.9-linux-amd64.tar.gz";
        let base_url = "https://releases.example.test/v9.9.9";

        // (a) Missing target arch asset in SHA256SUMS fails fast before downloading archive
        let http_missing = network::MockHttpClient::new();
        let other_digest = sha256_hex(b"arm64-only");
        http_missing.on_download(
            format!("{base_url}/SHA256SUMS"),
            200,
            format!("{other_digest}  debopty-v9.9.9-linux-arm64.tar.gz\n").into_bytes(),
        );
        let opts = SelfUpdateOptions {
            release_base_url: Some(base_url.to_string()),
            tag: Some(tag.to_string()),
            force: true,
            ..SelfUpdateOptions::default()
        };
        let err_missing = self_update(&ctx, &opts, &http_missing, &NoopRunner).unwrap_err();
        let msg_missing = err_missing.to_string();
        assert!(
            msg_missing.contains("SHA256SUMS 中缺少目标架构资产")
                && msg_missing.contains(asset_name),
            "unexpected missing-asset error: {msg_missing}"
        );
        assert_eq!(http_missing.requested_urls().len(), 1);

        // (b) Tampered archive bytes fail closed with "SHA-256 校验失败"
        let http_tampered = network::MockHttpClient::new();
        let expected_digest = sha256_hex(b"authentic-payload");
        http_tampered.on_download(
            format!("{base_url}/SHA256SUMS"),
            200,
            format!("{expected_digest}  {asset_name}\n").into_bytes(),
        );
        http_tampered.on_download(
            format!("{base_url}/{asset_name}"),
            200,
            b"\x1f\x8b\x08\x00tampered-payload".to_vec(),
        );
        let err_tampered = self_update(&ctx, &opts, &http_tampered, &NoopRunner).unwrap_err();
        let msg_tampered = err_tampered.to_string();
        assert!(
            msg_tampered.contains("SHA-256 校验失败"),
            "unexpected tampered error: {msg_tampered}"
        );
    }

    #[test]
    fn self_install_migrates_pre_rename_debopti_safely_and_preserves_custom_files() {
        let tmp = TempDir::new().expect("tempdir");
        let fake_bin = tmp.path().join("debopty-bin");
        fs::write(&fake_bin, b"#!/bin/sh\necho debopty\n").unwrap();
        fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).unwrap();

        // Case 1: Pre-rename symlink + legacy /opt/debopti + profiles + custom debopti.conf
        let root1 = tmp.path().join("root-symlink");
        fs::create_dir_all(root1.join("opt/debopti")).unwrap();
        fs::write(root1.join("opt/debopti/deb_optimizer.sh"), "#!/bin/bash\n").unwrap();
        fs::create_dir_all(root1.join("usr/local/bin")).unwrap();
        symlink(
            root1.join("opt/debopti/deb_optimizer.sh"),
            root1.join("usr/local/bin/debopti"),
        )
        .unwrap();
        fs::create_dir_all(root1.join("etc/profile.d")).unwrap();
        fs::create_dir_all(root1.join("etc/fish/conf.d")).unwrap();
        fs::write(
            root1.join("etc/profile.d/debopti.sh"),
            "alias debopti=old\n",
        )
        .unwrap();
        fs::write(
            root1.join("etc/fish/conf.d/debopti.fish"),
            "abbr debopti old\n",
        )
        .unwrap();
        fs::create_dir_all(root1.join("etc/debopti")).unwrap();
        fs::write(
            root1.join("etc/debopti/debopti.conf"),
            "# user comment\nIS_CN_REGION=\"true\"\nCUSTOM_KEY=\"keep\"\n",
        )
        .unwrap();

        let mut ctx1 = AppContext::with_root(&root1);
        ctx1.no_sys = true;
        let opts = SelfInstallOptions {
            source_bin: Some(fake_bin.clone()),
            ..SelfInstallOptions::default()
        };
        let rep1 = self_install(&ctx1, &opts, &NoopRunner).expect("self_install symlink");
        assert!(rep1.changed);
        assert!(rep1.details.contains_key("migrated_legacy_dir"));
        assert!(!root1.join("opt/debopti").exists());
        assert!(fs::symlink_metadata(root1.join("usr/local/bin/debopti")).is_err());
        assert!(!root1.join("etc/profile.d/debopti.sh").exists());
        assert!(!root1.join("etc/fish/conf.d/debopti.fish").exists());
        assert!(root1.join("etc/profile.d/debopty.sh").is_file());
        assert!(root1.join("etc/fish/conf.d/debopty.fish").is_file());
        assert!(root1.join("usr/local/bin/debopty").is_symlink());
        assert!(root1.join("usr/local/bin/debopty-f2b-sync").is_symlink());
        assert!(root1.join("usr/local/bin/debopty-rs").is_symlink());
        let conf1 = fs::read_to_string(root1.join("etc/debopti/debopti.conf")).unwrap();
        assert!(conf1.contains("IS_CN_REGION=\"true\""));
        assert!(conf1.contains("CUSTOM_KEY=\"keep\""));
        assert!(conf1.contains("# user comment"));

        // Case 2: Regular-file wrapper script at /usr/local/bin/debopti is removed
        let root2 = tmp.path().join("root-wrapper");
        fs::create_dir_all(root2.join("usr/local/bin")).unwrap();
        fs::write(
            root2.join("usr/local/bin/debopti"),
            "#!/bin/sh\nexec /opt/debopti/deb_optimizer.sh \"$@\"\n",
        )
        .unwrap();
        let mut ctx2 = AppContext::with_root(&root2);
        ctx2.no_sys = true;
        self_install(&ctx2, &opts, &NoopRunner).expect("self_install wrapper");
        assert!(!root2.join("usr/local/bin/debopti").exists());

        // Case 3: Unrelated user script at /usr/local/bin/debopti & markerless /opt/debopti are preserved (U8)
        let root3 = tmp.path().join("root-custom");
        fs::create_dir_all(root3.join("usr/local/bin")).unwrap();
        fs::write(
            root3.join("usr/local/bin/debopti"),
            "#!/bin/sh\necho user-tool\n",
        )
        .unwrap();
        fs::create_dir_all(root3.join("opt/debopti")).unwrap();
        fs::write(root3.join("opt/debopti/user_only.txt"), "keep-me").unwrap();
        let mut ctx3 = AppContext::with_root(&root3);
        ctx3.no_sys = true;
        let rep3 = self_install(&ctx3, &opts, &NoopRunner).expect("self_install custom");
        assert!(!rep3.details.contains_key("migrated_legacy_dir"));
        assert_eq!(
            fs::read_to_string(root3.join("usr/local/bin/debopti")).unwrap(),
            "#!/bin/sh\necho user-tool\n"
        );
        assert_eq!(
            fs::read_to_string(root3.join("opt/debopti/user_only.txt")).unwrap(),
            "keep-me"
        );
    }

    #[test]
    fn ensure_default_tools_skips_when_no_sys_is_set() {
        use crate::network::MockHttpClient;
        let tmp = TempDir::new().expect("tempdir");
        let mut ctx = AppContext::with_root(tmp.path());
        ctx.no_sys = true;
        let rep = ensure_default_tools(&ctx, &MockHttpClient::new(), &NoopRunner);
        assert!(!rep.changed);
        assert_eq!(rep.details.get("skipped").map(String::as_str), Some("true"));
    }
}
