use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;
use walkdir::WalkDir;

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

/// Verifies `file_path` against a `SHA256SUMS` manifest for `asset_name`.
pub fn verify_sha256sums(file_path: &Path, sums_text: &str, asset_name: &str) -> Result<bool> {
    if asset_name.trim().is_empty() || !file_path.is_file() {
        return Ok(false);
    }

    let mut expected: Option<String> = None;
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
            expected = Some(hash.to_ascii_lowercase());
            break;
        }
    }

    let Some(expected_hex) = expected else {
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

    // Migrate/remove legacy `debopti` shortcut and shell profile artifacts.
    let legacy_bin = ctx.path("/usr/local/bin/debopti");
    if let Ok(meta) = fs::symlink_metadata(&legacy_bin)
        && meta.file_type().is_symlink()
    {
        let should_remove = match fs::read_link(&legacy_bin) {
            Ok(target) => {
                let s = target.to_string_lossy();
                s.contains("deb_optimizer.sh") || s.contains("debopty") || !legacy_bin.exists()
            }
            Err(_) => true,
        };
        if should_remove {
            let _ = fs::remove_file(&legacy_bin);
        }
    }
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

    Ok(AppReport::new(
        MODULE_ID,
        "self-install",
        true,
        format!("debopty 原生二进制已安装至 {}", dest_bin.display()),
    )
    .with_detail("binary", dest_bin.to_string_lossy().into_owned())
    .with_detail("bin_link", bin_link.to_string_lossy().into_owned())
    .with_detail("f2b_bin_link", f2b_link.to_string_lossy().into_owned())
    .with_detail("rs_bin_link", rs_link.to_string_lossy().into_owned()))
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

    network::download_with_fallback(&sums_path, &format!("{base_url}/SHA256SUMS"), is_cn, http)
        .map_err(|e| app_err(format!("下载 SHA256SUMS 失败: {e}")))?;
    network::download_with_fallback(
        &archive_path,
        &format!("{base_url}/{asset_name}"),
        is_cn,
        http,
    )
    .map_err(|e| app_err(format!("下载发布包 {asset_name} 失败: {e}")))?;

    let sums_text = fs::read_to_string(&sums_path)?;
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
    if opts.keep_config {
        if conf_dir.exists() {
            report = report.with_preserved(format!("保留持久化配置目录 {}", conf_dir.display()));
        }
    } else if conf_dir.exists() {
        let _ = fs::remove_dir_all(&conf_dir);
        report = report.with_detail("purged_config_dir", conf_dir.to_string_lossy().into_owned());
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
        assert!(!root.join("opt/debopty/INDEX.md").exists());

        // Default uninstall preserves /etc/debopti (U8)
        let un_keep = self_uninstall(&ctx, &SelfUninstallOptions::default(), &runner)
            .expect("self_uninstall keep");
        assert!(un_keep.changed);
        assert!(!root.join("opt/debopty").exists());
        assert!(!root.join("usr/local/bin/debopty").exists());
        assert!(!root.join("usr/local/bin/debopty-f2b-sync").exists());
        assert!(root.join("etc/debopti/debopti.conf").is_file());
        assert!(!un_keep.preserved_user_custom.is_empty());

        // Purge uninstall removes /etc/debopti
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
    }
}
