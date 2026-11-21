use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;

use crate::config::{ConfigStore, DEFAULT_CONFIG_PATH};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::security::{SecurityContext, fail2ban};
use crate::sys::{self, CommandRunner};
use crate::template::TemplateEngine;

const APT_ENV: &[(&str, &str)] = &[("DEBIAN_FRONTEND", "noninteractive")];

const REDUNDANT_SERVICES: &[&str] = &["ModemManager", "avahi-daemon", "bluetooth", "cups", "pnmos"];

/// Upper bound for automatically created swapfile size (8 GiB).
pub const MAX_SWAP_MB: u64 = 8192;

#[derive(Debug, Clone)]
pub struct SystemContext {
    pub root_prefix: PathBuf,
    pub is_cn_region: bool,
    pub non_interactive: bool,
    pub no_sys: bool,
    pub config_path: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub tmux_conf_override: Option<PathBuf>,
    pub tmux_skip_apt: bool,
    pub tmux_version_override: Option<String>,
}

impl Default for SystemContext {
    fn default() -> Self {
        Self {
            root_prefix: PathBuf::from("/"),
            is_cn_region: false,
            non_interactive: false,
            no_sys: false,
            config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
            template_dir: None,
            tmux_conf_override: None,
            tmux_skip_apt: false,
            tmux_version_override: None,
        }
    }
}

impl SystemContext {
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let config_path = prefixed(&root, DEFAULT_CONFIG_PATH);
        Self {
            root_prefix: root,
            is_cn_region: false,
            non_interactive: false,
            no_sys: false,
            config_path,
            template_dir: None,
            tmux_conf_override: None,
            tmux_skip_apt: false,
            tmux_version_override: None,
        }
    }

    pub fn is_custom_root(&self) -> bool {
        !self.root_prefix.as_os_str().is_empty() && self.root_prefix != Path::new("/")
    }

    pub fn path(&self, abs: &str) -> PathBuf {
        prefixed(&self.root_prefix, abs)
    }

    pub fn effective_config_path(&self) -> PathBuf {
        if self.is_custom_root() && self.config_path == Path::new(DEFAULT_CONFIG_PATH) {
            self.path(DEFAULT_CONFIG_PATH)
        } else {
            self.config_path.clone()
        }
    }

    pub fn template_engine(&self) -> TemplateEngine {
        TemplateEngine::new(self.template_dir.clone())
    }

    pub fn to_security_context(&self) -> SecurityContext {
        let mut sec = SecurityContext::with_root(&self.root_prefix);
        sec.no_sys = self.no_sys;
        sec.non_interactive = self.non_interactive;
        sec.config_path = self.effective_config_path();
        sec.template_dir = self.template_dir.clone();
        sec
    }
}

/// Join an absolute system path beneath `root` when `root != "/"`.
pub fn prefixed(root: &Path, abs: &str) -> PathBuf {
    let clean = abs.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

fn ensure_trailing_newline(mut s: String) -> String {
    if !s.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn write_file_atomic(path: &Path, content: &[u8], mode: Option<u32>) -> Result<()> {
    sys::write_file_atomic(path, content, mode)
}

fn rewrite_apt_source_file(path: &Path, is_cn_region: bool, debian_ver: u32) -> Result<()> {
    if !path.is_file() {
        return Ok(());
    }
    let mut text = fs::read_to_string(path)?;
    let mut changed = false;

    if is_cn_region && debian_ver != 10 {
        let rewritten = text
            .replace("deb.debian.org", "mirrors.tuna.tsinghua.edu.cn")
            .replace("security.debian.org", "mirrors.tuna.tsinghua.edu.cn");
        if rewritten != text {
            text = rewritten;
            changed = true;
        }
    }

    if (debian_ver != 10 || is_cn_region) && text.contains("http://") {
        text = text.replace("http://", "https://");
        changed = true;
    }

    if changed {
        write_file_atomic(path, text.as_bytes(), None)?;
    }
    Ok(())
}

/// Configure APT mirrors (Debian 10 Buster EOL / Debian 11+ TUNA / DEB822 / HTTPS) and install all 16 base tools.
pub fn setup_base(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let sources_list = ctx.path("/etc/apt/sources.list");
    let sources_bak = ctx.path("/etc/apt/sources.list.bak");
    if sources_list.is_file() && !sources_bak.exists() {
        fs::copy(&sources_list, &sources_bak)?;
    }

    let debian_sources = ctx.path("/etc/apt/sources.list.d/debian.sources");
    let debian_sources_bak = ctx.path("/etc/apt/debian.sources.bak");
    if debian_sources.is_file() && !debian_sources_bak.exists() {
        fs::copy(&debian_sources, &debian_sources_bak)?;
    }

    let debian_ver = sys::detect_debian_major(&ctx.path("/etc/debian_version")).unwrap_or(0);
    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();

    if debian_ver == 10 {
        let tmpl = if ctx.is_cn_region {
            "templates/system/sources.list.buster.cn"
        } else {
            "templates/system/sources.list.buster.global"
        };
        let rendered = ensure_trailing_newline(engine.render_to_string(tmpl, &empty_vars)?);
        write_file_atomic(&sources_list, rendered.as_bytes(), Some(0o644))?;
    }

    if !ctx.no_sys {
        let has_ca = sys::is_dpkg_installed("ca-certificates", runner);
        if !has_ca {
            let _ = runner.run_with_env(
                "apt-get",
                &["update", "-yq"],
                APT_ENV,
                Duration::from_secs(120),
            );
            let _ = runner.run_with_env(
                "apt-get",
                &["install", "-yq", "ca-certificates"],
                APT_ENV,
                Duration::from_secs(180),
            );
        }
    }

    rewrite_apt_source_file(&sources_list, ctx.is_cn_region, debian_ver)?;
    if debian_ver != 10 {
        rewrite_apt_source_file(&debian_sources, ctx.is_cn_region, debian_ver)?;
    }

    let dns_pkg = if debian_ver >= 13 {
        "bind9-dnsutils"
    } else {
        "dnsutils"
    };
    let base_tools = [
        "curl",
        "wget",
        "gnupg",
        "lsb-release",
        "procps",
        "unzip",
        "tar",
        "openssl",
        "git",
        "logrotate",
        "whois",
        dns_pkg,
        "net-tools",
        "jq",
        "sudo",
        "tmux",
    ];

    if ctx.no_sys {
        return Ok(Vec::new());
    }

    let installed = sys::safe_apt_install(&base_tools, runner)?;
    if runner
        .run_with_env(
            "apt-get",
            &["upgrade", "-yq"],
            APT_ENV,
            Duration::from_secs(600),
        )
        .map(|o| o.success())
        .unwrap_or(false)
    {
        let _ = runner.run_with_env(
            "apt-get",
            &["autoremove", "-yq"],
            APT_ENV,
            Duration::from_secs(300),
        );
    }

    Ok(installed)
}

/// Optionally switch from generic kernel to Debian Cloud kernel (`amd64`/`arm64`) and purge old kernels.
///
/// Returns `true` if the cloud kernel was installed.
pub fn setup_kernel(
    ctx: &SystemContext,
    install_cloud: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    if ctx.no_sys {
        return Ok(false);
    }

    let uname_r = runner
        .run("uname", &["-r"], Duration::from_secs(5))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default();
    if uname_r.contains("cloud") {
        return Ok(false);
    }

    if !install_cloud.unwrap_or(false) {
        return Ok(false);
    }

    let arch = runner
        .run("uname", &["-m"], Duration::from_secs(5))
        .ok()
        .filter(|o| o.success() && !o.stdout.trim().is_empty())
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let flavor = sys::detect_arch_kernel_flavor(&arch);
    let img_pkg = format!("linux-image-cloud-{flavor}");
    let hdr_pkg = format!("linux-headers-cloud-{flavor}");

    let out = runner.run_with_env(
        "apt-get",
        &["install", "-yq", &img_pkg, &hdr_pkg],
        APT_ENV,
        Duration::from_secs(600),
    )?;
    if !out.success() {
        return Err(CoreError::System(format!(
            "cloud kernel install failed: {}",
            out.stderr.trim()
        )));
    }

    let grub_ok = runner
        .run("update-grub", &[], Duration::from_secs(120))
        .map(|o| o.success())
        .unwrap_or(false);

    // Never purge old kernels if `uname -r` is empty or `update-grub` failed.
    if !uname_r.is_empty()
        && grub_ok
        && let Ok(dpkg_out) = runner.run("dpkg", &["-l"], Duration::from_secs(15))
        && dpkg_out.success()
    {
        let old_kernels: Vec<String> = dpkg_out
            .stdout
            .lines()
            .filter_map(|line| {
                let mut cols = line.split_whitespace();
                let state = cols.next()?;
                let pkg = cols.next()?;
                if state == "ii"
                    && pkg.contains("linux-image")
                    && !pkg.contains("cloud")
                    && !pkg.contains(&uname_r)
                {
                    Some(pkg.to_string())
                } else {
                    None
                }
            })
            .collect();

        if !old_kernels.is_empty() {
            let mut purge_args: Vec<&str> = Vec::with_capacity(2 + old_kernels.len());
            purge_args.push("purge");
            purge_args.push("-yq");
            for k in &old_kernels {
                purge_args.push(k.as_str());
            }
            let _ = runner.run_with_env("apt-get", &purge_args, APT_ENV, Duration::from_secs(300));
        }
    }

    Ok(true)
}

/// Render BBR/TCP sysctl optimization config and apply via `sysctl --system` (container-safe).
pub fn setup_sysctl(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<()> {
    let engine = ctx.template_engine();
    let rendered = ensure_trailing_newline(engine.render_to_string(
        "templates/system/99-debopti-optimize.conf",
        &BTreeMap::new(),
    )?);
    write_file_atomic(
        &ctx.path("/etc/sysctl.d/99-debopti-optimize.conf"),
        rendered.as_bytes(),
        Some(0o644),
    )?;

    if !ctx.no_sys {
        let _ = runner.run("sysctl", &["--system"], Duration::from_secs(30));
    }

    Ok(())
}

fn ensure_systemd_manager_header(path: &Path) -> Result<()> {
    let existing = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };
    if !existing.lines().any(|l| l.trim() == "[Manager]") {
        let mut next = String::from("[Manager]\n");
        if !existing.is_empty() {
            next.push_str(&existing);
            if !next.ends_with('\n') {
                next.push('\n');
            }
        }
        write_file_atomic(path, next.as_bytes(), None)?;
    }
    Ok(())
}

/// Configure `nofile` limits in `/etc/security/limits.d` and Systemd global unit configs.
pub fn setup_limits(ctx: &SystemContext) -> Result<()> {
    let engine = ctx.template_engine();
    let rendered = ensure_trailing_newline(
        engine.render_to_string("templates/system/99-debopti-nofile.conf", &BTreeMap::new())?,
    );
    write_file_atomic(
        &ctx.path("/etc/security/limits.d/99-debopti-nofile.conf"),
        rendered.as_bytes(),
        Some(0o644),
    )?;

    let sys_conf = ctx.path("/etc/systemd/system.conf");
    let user_conf = ctx.path("/etc/systemd/user.conf");
    ensure_systemd_manager_header(&sys_conf)?;
    ensure_systemd_manager_header(&user_conf)?;

    sys::set_conf_value(&sys_conf, "DefaultLimitNOFILE", "1048576", "=")?;
    sys::set_conf_value(&user_conf, "DefaultLimitNOFILE", "1048576", "=")?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryOptions {
    pub enable_zram: bool,
    pub enable_swap: bool,
}

impl Default for MemoryOptions {
    fn default() -> Self {
        Self {
            enable_zram: true,
            enable_swap: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryReport {
    pub mem_total_mb: u64,
    pub zram_configured: bool,
    pub swap_created: bool,
    pub swap_size_mb: u64,
}

/// Configure ZRAM compression (`zram-tools`) and/or a capped 2x-RAM physical `/swapfile` (max 8 GiB).
pub fn setup_memory(
    ctx: &SystemContext,
    opts: &MemoryOptions,
    runner: &dyn CommandRunner,
) -> Result<MemoryReport> {
    let mem_mb = sys::read_mem_total_mb(&ctx.path("/proc/meminfo")).unwrap_or(0);
    let mut zram_configured = false;
    let mut swap_created = false;
    let mut swap_size_mb = 0;

    if opts.enable_zram {
        if !ctx.no_sys {
            let _ = runner.run_with_env(
                "apt-get",
                &["install", "-yq", "zram-tools"],
                APT_ENV,
                Duration::from_secs(180),
            );
        }
        let engine = ctx.template_engine();
        let rendered = ensure_trailing_newline(
            engine.render_to_string("templates/system/zramswap", &BTreeMap::new())?,
        );
        write_file_atomic(
            &ctx.path("/etc/default/zramswap"),
            rendered.as_bytes(),
            Some(0o644),
        )?;
        if !ctx.no_sys {
            let _ = runner.run(
                "systemctl",
                &["restart", "zramswap"],
                Duration::from_secs(15),
            );
        }
        zram_configured = true;
    }

    if opts.enable_swap {
        let swaps_txt = fs::read_to_string(ctx.path("/proc/swaps")).unwrap_or_default();
        if !swaps_txt.contains("/swapfile") && !ctx.no_sys && mem_mb > 0 {
            let target_mb = mem_mb.saturating_mul(2).min(MAX_SWAP_MB);
            let swapfile = ctx.path("/swapfile");
            let swap_str = swapfile.to_string_lossy().into_owned();
            let size_arg = format!("{target_mb}M");

            let alloc_ok = runner
                .run(
                    "fallocate",
                    &["-l", &size_arg, &swap_str],
                    Duration::from_secs(120),
                )
                .map(|o| o.success())
                .unwrap_or(false)
                || {
                    let of_arg = format!("of={swap_str}");
                    let count_arg = format!("count={target_mb}");
                    runner
                        .run(
                            "dd",
                            &["if=/dev/zero", &of_arg, "bs=1M", &count_arg],
                            Duration::from_secs(300),
                        )
                        .map(|o| o.success())
                        .unwrap_or(false)
                };

            if !alloc_ok {
                if swapfile.exists() {
                    let _ = fs::remove_file(&swapfile);
                }
            } else {
                if swapfile.is_file() {
                    let _ = fs::set_permissions(&swapfile, fs::Permissions::from_mode(0o600));
                } else {
                    let _ = runner.run("chmod", &["600", &swap_str], Duration::from_secs(10));
                }

                let mkswap_ok = runner
                    .run("mkswap", &[&swap_str], Duration::from_secs(60))
                    .map(|o| o.success())
                    .unwrap_or(false);
                let swapon_ok = mkswap_ok
                    && runner
                        .run("swapon", &[&swap_str], Duration::from_secs(30))
                        .map(|o| o.success())
                        .unwrap_or(false);

                if swapon_ok {
                    let fstab = ctx.path("/etc/fstab");
                    let existing = fs::read_to_string(&fstab).unwrap_or_default();
                    if !existing.contains("/swapfile") {
                        let mut next = if existing.is_empty() {
                            String::new()
                        } else {
                            ensure_trailing_newline(existing)
                        };
                        next.push_str("/swapfile none swap sw 0 0\n");
                        write_file_atomic(&fstab, next.as_bytes(), None)?;
                    }
                    swap_created = true;
                    swap_size_mb = target_mb;
                } else if swapfile.exists() {
                    let _ = fs::remove_file(&swapfile);
                }
            }
        }
    }

    Ok(MemoryReport {
        mem_total_mb: mem_mb,
        zram_configured,
        swap_created,
        swap_size_mb,
    })
}

/// Low-memory VPS optimization: reduce TTYs, purge `rsyslog`, cap `journald`, optionally mask services.
///
/// Returns the list of masked service names.
pub fn setup_low_memory_optimization(
    ctx: &SystemContext,
    mask_services: bool,
    runner: &dyn CommandRunner,
) -> Result<Vec<String>> {
    let logind = ctx.path("/etc/systemd/logind.conf");
    if logind.is_file() {
        sys::set_conf_value(&logind, "NAutoVTs", "2", "=")?;
        sys::set_conf_value(&logind, "ReserveVT", "2", "=")?;
        if !ctx.no_sys {
            let _ = runner.run(
                "systemctl",
                &["restart", "systemd-logind"],
                Duration::from_secs(15),
            );
        }
    }

    if !ctx.no_sys && sys::is_dpkg_installed("rsyslog", runner) {
        let _ = runner.run_with_env(
            "apt-get",
            &["purge", "-yq", "rsyslog"],
            APT_ENV,
            Duration::from_secs(120),
        );
    }

    let journald = ctx.path("/etc/systemd/journald.conf");
    if journald.is_file() {
        sys::set_conf_value(&journald, "SystemMaxUse", "200M", "=")?;
        sys::set_conf_value(&journald, "RuntimeMaxUse", "10M", "=")?;
        if !ctx.no_sys {
            let _ = runner.run(
                "systemctl",
                &["restart", "systemd-journald"],
                Duration::from_secs(15),
            );
        }
    }

    let mut masked = Vec::new();
    if mask_services
        && !ctx.no_sys
        && let Ok(units) = runner.run("systemctl", &["list-unit-files"], Duration::from_secs(15))
        && units.success()
    {
        for svc in REDUNDANT_SERVICES {
            let unit_prefix = format!("{svc}.service");
            if units
                .stdout
                .lines()
                .any(|l| l.trim_start().starts_with(&unit_prefix))
            {
                let _ = runner.run("systemctl", &["stop", svc], Duration::from_secs(15));
                let _ = runner.run("systemctl", &["mask", svc], Duration::from_secs(15));
                masked.push((*svc).to_string());
            }
        }
    }

    Ok(masked)
}

fn logrotate_freq_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*#?\s*(daily|weekly|monthly|yearly)\s*$")
            .expect("valid logrotate frequency regex")
    })
}

/// Update `/etc/logrotate.conf` to `daily` (replacing any `weekly`/`monthly`/`yearly`), `rotate 7`, `compress`.
pub fn setup_logrotate(ctx: &SystemContext) -> Result<()> {
    let conf = ctx.path("/etc/logrotate.conf");
    let existing = if conf.is_file() {
        fs::read_to_string(&conf)?
    } else {
        String::new()
    };

    let freq_re = logrotate_freq_re();
    let mut placed_daily = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if freq_re.is_match(line) {
            if !placed_daily {
                out_lines.push("daily".to_string());
                placed_daily = true;
            }
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !placed_daily {
        if let Some(inc_pos) = out_lines
            .iter()
            .position(|l| l.trim_start().starts_with("include "))
        {
            out_lines.insert(inc_pos, "daily".to_string());
        } else {
            out_lines.push("daily".to_string());
        }
    }

    let mut rendered = out_lines.join("\n");
    rendered.push('\n');
    write_file_atomic(&conf, rendered.as_bytes(), None)?;

    sys::set_conf_value(&conf, "rotate", "7", " ")?;
    sys::set_conf_value(&conf, "compress", "", "")?;
    Ok(())
}

/// Set system timezone to `Asia/Shanghai` and enable `chrony`.
pub fn setup_timezone(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<()> {
    if !ctx.no_sys {
        let _ = runner.run(
            "timedatectl",
            &["set-timezone", "Asia/Shanghai"],
            Duration::from_secs(10),
        );
        let _ = runner.run_with_env(
            "apt-get",
            &["install", "-yq", "chrony"],
            APT_ENV,
            Duration::from_secs(180),
        );
        let _ = runner.run(
            "systemctl",
            &["enable", "--now", "chrony"],
            Duration::from_secs(15),
        );
    }
    Ok(())
}

fn tmux_ver_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"([0-9]+)\.([0-9]+)").expect("valid tmux version regex"))
}

/// Parse `(major, minor)` from tmux version strings like `"tmux 3.1c"`, `"tmux 2.8"`, `"3.5a"`.
pub fn parse_tmux_version(raw: &str) -> Option<(u32, u32)> {
    let caps = tmux_ver_re().captures(raw)?;
    let major = caps[1].parse::<u32>().ok()?;
    let minor = caps[2].parse::<u32>().ok()?;
    Some((major, minor))
}

/// Return `true` when `cur >= req`.
pub fn tmux_version_ge(cur: (u32, u32), req: (u32, u32)) -> bool {
    cur.0 > req.0 || (cur.0 == req.0 && cur.1 >= req.1)
}

fn resolve_tmux_target(ctx: &SystemContext) -> PathBuf {
    if let Some(ref p) = ctx.tmux_conf_override {
        return p.clone();
    }
    if let Ok(env_p) = env::var("DEBOPTY_TMUX_CONF")
        && !env_p.is_empty()
    {
        return PathBuf::from(env_p);
    }
    ctx.path("/etc/tmux.conf")
}

fn resolve_tmux_skip_apt(ctx: &SystemContext) -> bool {
    ctx.tmux_skip_apt
        || ctx.no_sys
        || env::var("DEBOPTY_TMUX_SKIP_APT").ok().as_deref() == Some("1")
}

/// Deploy version-gated global tmux configuration with backup & idempotency (`tests/tmux-test.sh` parity).
pub fn setup_tmux(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<PathBuf> {
    let target = resolve_tmux_target(ctx);
    let backup = PathBuf::from(format!("{}.debopty.bak", target.display()));

    if !resolve_tmux_skip_apt(ctx) {
        sys::safe_apt_install(&["tmux"], runner)?;
    }

    if target.is_file() {
        let existing = fs::read_to_string(&target).unwrap_or_default();
        if !existing.contains("# managed-by-debopty") && !backup.exists() {
            let meta = fs::metadata(&target)?;
            fs::copy(&target, &backup)?;
            let _ = fs::set_permissions(&backup, meta.permissions());
        }
    }

    let default_term = if runner
        .run("infocmp", &["tmux-256color"], Duration::from_secs(5))
        .map(|o| o.success())
        .unwrap_or(false)
    {
        "tmux-256color"
    } else {
        "screen-256color"
    };

    let raw_ver = if let Some(ref ov) = ctx.tmux_version_override {
        ov.clone()
    } else if let Ok(env_v) = env::var("DEBOPTY_TMUX_VERSION_OVERRIDE")
        && !env_v.is_empty()
    {
        env_v
    } else {
        runner
            .run("tmux", &["-V"], Duration::from_secs(5))
            .ok()
            .filter(|o| o.success())
            .map(|o| o.stdout)
            .unwrap_or_default()
    };

    let mut modern_features = String::new();
    if let Some(ver) = parse_tmux_version(&raw_ver) {
        if tmux_version_ge(ver, (3, 2)) {
            modern_features.push_str(r#"set -as terminal-features ",*:RGB""#);
        }
        if tmux_version_ge(ver, (3, 3)) {
            if !modern_features.is_empty() {
                modern_features.push('\n');
            }
            modern_features.push_str("set -g allow-passthrough on");
        }
    }

    let engine = ctx.template_engine();
    let mut vars = BTreeMap::new();
    vars.insert("TMUX_DEFAULT_TERM".to_string(), default_term.to_string());
    vars.insert("TMUX_MODERN_FEATURES".to_string(), modern_features);
    let rendered =
        ensure_trailing_newline(engine.render_to_string("templates/system/tmux.conf", &vars)?);
    write_file_atomic(&target, rendered.as_bytes(), Some(0o644))?;

    if target == Path::new("/etc/tmux.conf")
        && !ctx.no_sys
        && runner
            .run("tmux", &["list-sessions"], Duration::from_secs(5))
            .map(|o| o.success())
            .unwrap_or(false)
    {
        let _ = runner.run(
            "tmux",
            &["source-file", "/etc/tmux.conf"],
            Duration::from_secs(5),
        );
    }

    Ok(target)
}

/// Restore backed-up tmux config or remove managed config, and uninstall `tmux` unless inside `$TMUX`.
pub fn uninstall_tmux(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<()> {
    let target = resolve_tmux_target(ctx);
    let backup = PathBuf::from(format!("{}.debopty.bak", target.display()));

    if backup.is_file() {
        fs::rename(&backup, &target)?;
    } else if target.is_file() {
        let existing = fs::read_to_string(&target).unwrap_or_default();
        if existing.contains("# managed-by-debopty") {
            fs::remove_file(&target)?;
        }
    }

    if !resolve_tmux_skip_apt(ctx) {
        let in_tmux = env::var("TMUX").ok().is_some_and(|v| !v.is_empty());
        if !in_tmux && sys::is_dpkg_installed("tmux", runner) {
            let _ = runner.run_with_env(
                "apt-get",
                &["remove", "-yq", "tmux"],
                APT_ENV,
                Duration::from_secs(180),
            );
        }
    }

    Ok(())
}

/// Toggle IPv4/IPv6 forwarding (`true` = enabled after toggle, `false` = disabled after toggle).
pub fn toggle_ip_forwarding(ctx: &SystemContext, runner: &dyn CommandRunner) -> Result<bool> {
    let conf = ctx.path("/etc/sysctl.d/99-debopti-forwarding.conf");

    let currently_enabled = if ctx.no_sys {
        conf.is_file()
    } else {
        runner
            .run(
                "sysctl",
                &["-n", "net.ipv4.ip_forward"],
                Duration::from_secs(5),
            )
            .map(|o| o.success() && o.stdout.trim() == "1")
            .unwrap_or(false)
    };

    if currently_enabled {
        if conf.exists() {
            fs::remove_file(&conf)?;
        }
        if !ctx.no_sys {
            for kv in [
                "net.ipv4.ip_forward=0",
                "net.ipv4.conf.all.forwarding=0",
                "net.ipv4.conf.default.forwarding=0",
                "net.ipv6.conf.all.forwarding=0",
                "net.ipv6.conf.default.forwarding=0",
            ] {
                let _ = runner.run("sysctl", &["-w", kv], Duration::from_secs(10));
            }
        }
        Ok(false)
    } else {
        let engine = ctx.template_engine();
        let rendered = ensure_trailing_newline(engine.render_to_string(
            "templates/system/99-debopti-forwarding.conf",
            &BTreeMap::new(),
        )?);
        write_file_atomic(&conf, rendered.as_bytes(), Some(0o644))?;
        if !ctx.no_sys {
            let _ = runner.run("sysctl", &["--system"], Duration::from_secs(30));
        }
        Ok(true)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseOptimizationReport {
    pub is_cn_region: bool,
    pub base_optimized: bool,
}

/// Orchestrate the full base system optimization workflow matching `scripts/system.sh::run_base_optimization`.
pub fn run_base_optimization(
    ctx: &SystemContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<BaseOptimizationReport> {
    let store = ConfigStore::new(ctx.effective_config_path());
    let net_report = network::global_netcheck(&store, false, http)?;

    let mut eff_ctx = ctx.clone();
    eff_ctx.is_cn_region = net_report.is_cn_region;

    setup_base(&eff_ctx, runner)?;
    setup_kernel(&eff_ctx, None, runner)?;
    setup_sysctl(&eff_ctx, runner)?;
    setup_limits(&eff_ctx)?;

    let sec_ctx = eff_ctx.to_security_context();
    fail2ban::setup_security(&sec_ctx, runner)?;

    setup_tmux(&eff_ctx, runner)?;
    setup_memory(&eff_ctx, &MemoryOptions::default(), runner)?;
    setup_low_memory_optimization(&eff_ctx, true, runner)?;
    setup_logrotate(&eff_ctx)?;
    setup_timezone(&eff_ctx, runner)?;

    store.set("BASE_OPTIMIZED", "true")?;

    Ok(BaseOptimizationReport {
        is_cn_region: net_report.is_cn_region,
        base_optimized: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::{HttpResponse, MockHttpClient};
    use crate::sys::{CmdOutput, MockCommandRunner};
    use tempfile::TempDir;

    fn ok_stdout(s: &str) -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: s.to_string(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_out() -> CmdOutput {
        CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    #[test]
    fn setup_base_handles_buster_eol_cn_tuna_and_debian_13_bind9() {
        // Case 1: Debian 10 (Buster) EOL Global keeps http:// archive
        let tmp10 = TempDir::new().unwrap();
        let mut ctx10 = SystemContext::with_root(tmp10.path());
        ctx10.is_cn_region = false;
        ctx10.no_sys = true;
        fs::create_dir_all(ctx10.path("/etc/apt")).unwrap();
        fs::write(ctx10.path("/etc/debian_version"), "10.13\n").unwrap();
        fs::write(
            ctx10.path("/etc/apt/sources.list"),
            "deb http://deb.debian.org/debian buster main\n",
        )
        .unwrap();

        let runner = MockCommandRunner::new();
        setup_base(&ctx10, &runner).unwrap();
        assert!(ctx10.path("/etc/apt/sources.list.bak").is_file());
        let list10 = fs::read_to_string(ctx10.path("/etc/apt/sources.list")).unwrap();
        assert!(list10.contains("http://archive.debian.org/debian"));

        // Case 2: Debian 13 CN rewrites to TUNA HTTPS and selects bind9-dnsutils
        let tmp13 = TempDir::new().unwrap();
        let mut ctx13 = SystemContext::with_root(tmp13.path());
        ctx13.is_cn_region = true;
        ctx13.no_sys = false;
        fs::create_dir_all(ctx13.path("/etc/apt")).unwrap();
        fs::write(ctx13.path("/etc/debian_version"), "13.0\n").unwrap();
        let sources13 = ctx13.path("/etc/apt/sources.list");
        fs::write(
            &sources13,
            "deb http://deb.debian.org/debian trixie main\ndeb http://security.debian.org/debian-security trixie-security main\n",
        )
        .unwrap();
        fs::set_permissions(&sources13, fs::Permissions::from_mode(0o644)).unwrap();

        let runner13 = MockCommandRunner::new();
        runner13.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "ca-certificates"],
            ok_stdout("install ok installed"),
        );
        runner13.on_prog("dpkg-query", err_out());
        runner13.on_prog("apt-get", ok_stdout(""));

        let installed = setup_base(&ctx13, &runner13).unwrap();
        assert_eq!(installed.len(), 16);
        assert!(installed.contains(&"bind9-dnsutils".to_string()));
        assert!(!installed.contains(&"dnsutils".to_string()));

        let list13 = fs::read_to_string(ctx13.path("/etc/apt/sources.list")).unwrap();
        assert!(list13.contains("https://mirrors.tuna.tsinghua.edu.cn/debian"));
        assert!(!list13.contains("http://"));
        assert!(!list13.contains("deb.debian.org"));
        assert_eq!(
            fs::metadata(ctx13.path("/etc/apt/sources.list"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn setup_kernel_installs_cloud_and_purges_old_kernels() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());

        // Already cloud -> skip
        let runner_cloud = MockCommandRunner::new();
        runner_cloud.on_exact("uname", &["-r"], ok_stdout("6.1.0-21-cloud-amd64\n"));
        assert!(!setup_kernel(&ctx, Some(true), &runner_cloud).unwrap());

        // Physical kernel on aarch64 -> installs arm64 cloud kernel & purges old
        let runner = MockCommandRunner::new();
        runner.on_exact("uname", &["-r"], ok_stdout("6.1.0-21-arm64\n"));
        runner.on_exact("uname", &["-m"], ok_stdout("aarch64\n"));
        runner.on_prog("apt-get", ok_stdout(""));
        runner.on_exact("update-grub", &[], ok_stdout(""));
        runner.on_exact(
            "dpkg",
            &["-l"],
            ok_stdout(
                "ii  linux-image-6.1.0-18-arm64       6.1.76-1  arm64\n\
                 ii  linux-image-6.1.0-21-arm64       6.1.90-1  arm64\n\
                 ii  linux-image-cloud-arm64          6.1.90-1  arm64\n",
            ),
        );

        assert!(setup_kernel(&ctx, Some(true), &runner).unwrap());
        let env_calls = runner.env_invocations();
        assert_eq!(
            env_calls[0].1,
            vec![
                "install",
                "-yq",
                "linux-image-cloud-arm64",
                "linux-headers-cloud-arm64"
            ]
        );
        assert_eq!(
            env_calls[1].1,
            vec!["purge", "-yq", "linux-image-6.1.0-18-arm64"]
        );
    }

    #[test]
    fn sysctl_limits_logrotate_timezone_and_low_mem() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        fs::create_dir_all(ctx.path("/etc/systemd")).unwrap();
        fs::write(ctx.path("/etc/systemd/logind.conf"), "#NAutoVTs=6\n").unwrap();
        fs::write(ctx.path("/etc/systemd/journald.conf"), "#SystemMaxUse=\n").unwrap();
        fs::write(
            ctx.path("/etc/logrotate.conf"),
            "weekly\nrotate 4\n#compress\n",
        )
        .unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("sysctl", ok_stdout(""));
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "rsyslog"],
            ok_stdout("install ok installed"),
        );
        runner.on_prog("apt-get", ok_stdout(""));
        runner.on_prog("timedatectl", ok_stdout(""));
        runner.on_exact(
            "systemctl",
            &["list-unit-files"],
            ok_stdout("ModemManager.service enabled\ncups.service disabled\n"),
        );
        runner.on_prog("systemctl", ok_stdout(""));

        setup_sysctl(&ctx, &runner).unwrap();
        assert!(
            fs::read_to_string(ctx.path("/etc/sysctl.d/99-debopti-optimize.conf"))
                .unwrap()
                .contains("net.ipv4.tcp_congestion_control = bbr")
        );

        setup_limits(&ctx).unwrap();
        assert!(
            fs::read_to_string(ctx.path("/etc/security/limits.d/99-debopti-nofile.conf"))
                .unwrap()
                .contains("1048576")
        );
        assert!(
            fs::read_to_string(ctx.path("/etc/systemd/system.conf"))
                .unwrap()
                .contains("DefaultLimitNOFILE=1048576")
        );
        assert!(
            fs::read_to_string(ctx.path("/etc/systemd/user.conf"))
                .unwrap()
                .contains("DefaultLimitNOFILE=1048576")
        );

        let masked = setup_low_memory_optimization(&ctx, true, &runner).unwrap();
        assert_eq!(masked, vec!["ModemManager", "cups"]);
        assert!(
            fs::read_to_string(ctx.path("/etc/systemd/logind.conf"))
                .unwrap()
                .contains("NAutoVTs=2")
        );
        assert!(
            fs::read_to_string(ctx.path("/etc/systemd/journald.conf"))
                .unwrap()
                .contains("SystemMaxUse=200M")
        );

        setup_logrotate(&ctx).unwrap();
        let lr = fs::read_to_string(ctx.path("/etc/logrotate.conf")).unwrap();
        assert!(lr.contains("daily"));
        assert!(lr.contains("rotate 7"));
        assert!(lr.contains("compress"));

        setup_timezone(&ctx, &runner).unwrap();
        assert!(runner.invocations().iter().any(|(p, a)| {
            p == "timedatectl"
                && a == &vec!["set-timezone".to_string(), "Asia/Shanghai".to_string()]
        }));
    }

    #[test]
    fn setup_memory_zram_swap_and_lxc_cleanup() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        fs::create_dir_all(ctx.path("/proc")).unwrap();
        fs::write(ctx.path("/proc/meminfo"), "MemTotal:        1048576 kB\n").unwrap();
        fs::write(ctx.path("/proc/swaps"), "Filename\tType\tSize\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("apt-get", ok_stdout(""));
        runner.on_prog("systemctl", ok_stdout(""));
        runner.on_prog("fallocate", ok_stdout(""));
        runner.on_prog("chmod", ok_stdout(""));
        runner.on_prog("mkswap", ok_stdout(""));
        runner.on_prog("swapon", ok_stdout(""));

        let rep = setup_memory(&ctx, &MemoryOptions::default(), &runner).unwrap();
        assert_eq!(rep.mem_total_mb, 1024);
        assert!(rep.zram_configured);
        assert!(rep.swap_created);
        assert_eq!(rep.swap_size_mb, 2048);
        assert!(
            fs::read_to_string(ctx.path("/etc/default/zramswap"))
                .unwrap()
                .contains("ALGO=zstd")
        );
        assert!(
            fs::read_to_string(ctx.path("/etc/fstab"))
                .unwrap()
                .contains("/swapfile none swap sw 0 0")
        );

        // LXC failure removes /swapfile cleanly
        let tmp2 = TempDir::new().unwrap();
        let ctx2 = SystemContext::with_root(tmp2.path());
        fs::create_dir_all(ctx2.path("/proc")).unwrap();
        fs::write(ctx2.path("/proc/meminfo"), "MemTotal:        524288 kB\n").unwrap();
        fs::write(ctx2.path("/swapfile"), b"dummy").unwrap();

        let runner_lxc = MockCommandRunner::new();
        runner_lxc.on_prog("fallocate", ok_stdout(""));
        runner_lxc.on_prog("mkswap", ok_stdout(""));
        runner_lxc.on_prog("swapon", err_out());

        let rep2 = setup_memory(
            &ctx2,
            &MemoryOptions {
                enable_zram: false,
                enable_swap: true,
            },
            &runner_lxc,
        )
        .unwrap();
        assert!(!rep2.swap_created);
        assert!(!ctx2.path("/swapfile").exists());
    }

    #[test]
    fn tmux_version_gating_backup_idempotency_and_uninstall() {
        assert_eq!(parse_tmux_version("tmux 3.1c"), Some((3, 1)));
        assert_eq!(parse_tmux_version("tmux 2.8"), Some((2, 8)));
        assert_eq!(parse_tmux_version("3.5a"), Some((3, 5)));
        assert!(tmux_version_ge((3, 3), (3, 2)));
        assert!(!tmux_version_ge((3, 1), (3, 2)));

        let tmp = TempDir::new().unwrap();
        let runner = MockCommandRunner::new();
        runner.on_exact("infocmp", &["tmux-256color"], ok_stdout(""));

        // 2.8 omits modern features
        let conf28 = tmp.path().join("tmux-2.8.conf");
        let mut ctx28 = SystemContext::with_root(tmp.path());
        ctx28.tmux_conf_override = Some(conf28.clone());
        ctx28.tmux_skip_apt = true;
        ctx28.tmux_version_override = Some("tmux 2.8".to_string());
        setup_tmux(&ctx28, &runner).unwrap();
        let txt28 = fs::read_to_string(&conf28).unwrap();
        assert!(txt28.contains("# managed-by-debopty"));
        assert!(txt28.contains(r#"set -g default-terminal "tmux-256color""#));
        assert!(!txt28.contains("terminal-features"));
        assert!(!txt28.contains("allow-passthrough"));

        // 3.5a includes both terminal-features and allow-passthrough
        let conf35 = tmp.path().join("tmux-3.5a.conf");
        let mut ctx35 = ctx28.clone();
        ctx35.tmux_conf_override = Some(conf35.clone());
        ctx35.tmux_version_override = Some("tmux 3.5a".to_string());
        setup_tmux(&ctx35, &runner).unwrap();
        let txt35 = fs::read_to_string(&conf35).unwrap();
        assert!(txt35.contains(r#"set -as terminal-features ",*:RGB""#));
        assert!(txt35.contains("set -g allow-passthrough on"));

        // Backup + idempotency + restore on uninstall
        let custom = tmp.path().join("custom.conf");
        let backup = tmp.path().join("custom.conf.debopty.bak");
        fs::write(&custom, "# user custom\nset -g prefix C-a\n").unwrap();

        let mut ctx_custom = ctx35.clone();
        ctx_custom.tmux_conf_override = Some(custom.clone());
        setup_tmux(&ctx_custom, &runner).unwrap();
        assert!(backup.is_file());
        assert!(fs::read_to_string(&backup).unwrap().contains("prefix C-a"));
        assert!(
            fs::read_to_string(&custom)
                .unwrap()
                .contains("# managed-by-debopty")
        );

        // Idempotent re-run preserves backup
        setup_tmux(&ctx_custom, &runner).unwrap();
        assert!(fs::read_to_string(&backup).unwrap().contains("prefix C-a"));

        // Uninstall restores backup
        uninstall_tmux(&ctx_custom, &runner).unwrap();
        assert!(!backup.exists());
        assert!(fs::read_to_string(&custom).unwrap().contains("prefix C-a"));

        // Uninstall on unmanaged file without backup leaves it untouched
        uninstall_tmux(&ctx_custom, &runner).unwrap();
        assert!(custom.is_file());

        // Uninstall on managed file without backup deletes it
        uninstall_tmux(&ctx35, &runner).unwrap();
        assert!(!conf35.exists());
    }

    #[test]
    fn toggle_ip_forwarding_switches_on_and_off() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());

        let runner_off = MockCommandRunner::new();
        runner_off.on_exact("sysctl", &["-n", "net.ipv4.ip_forward"], ok_stdout("0\n"));
        runner_off.on_exact("sysctl", &["--system"], ok_stdout(""));

        assert!(toggle_ip_forwarding(&ctx, &runner_off).unwrap());
        let fwd = ctx.path("/etc/sysctl.d/99-debopti-forwarding.conf");
        assert!(fwd.is_file());
        assert!(
            fs::read_to_string(&fwd)
                .unwrap()
                .contains("net.ipv4.ip_forward = 1")
        );

        let runner_on = MockCommandRunner::new();
        runner_on.on_exact("sysctl", &["-n", "net.ipv4.ip_forward"], ok_stdout("1\n"));
        runner_on.on_exact("sysctl", &["-w", "net.ipv4.ip_forward=0"], ok_stdout(""));

        assert!(!toggle_ip_forwarding(&ctx, &runner_on).unwrap());
        assert!(!fwd.exists());
    }

    #[test]
    fn run_base_optimization_persists_base_optimized_flag() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SystemContext::with_root(tmp.path());
        ctx.no_sys = true;
        ctx.non_interactive = true;

        fs::create_dir_all(ctx.path("/etc/apt")).unwrap();
        fs::write(ctx.path("/etc/debian_version"), "12.8\n").unwrap();
        fs::write(
            ctx.path("/etc/apt/sources.list"),
            "deb http://deb.debian.org/debian bookworm main\n",
        )
        .unwrap();

        let http = MockHttpClient::new();
        http.on_get(
            "https://ipinfo.io/country",
            HttpResponse {
                status: 200,
                body: "US\n".to_string(),
            },
        );
        let runner = MockCommandRunner::new();

        let rep = run_base_optimization(&ctx, &http, &runner).unwrap();
        assert!(!rep.is_cn_region);
        assert!(rep.base_optimized);

        let store = ConfigStore::new(ctx.effective_config_path());
        assert_eq!(
            store.get("BASE_OPTIMIZED").unwrap().as_deref(),
            Some("true")
        );
        assert_eq!(store.get("IS_CN_REGION").unwrap().as_deref(), Some("false"));
        assert!(ctx.path("/etc/sysctl.d/99-debopti-optimize.conf").is_file());
        assert!(
            ctx.path("/etc/security/limits.d/99-debopti-nofile.conf")
                .is_file()
        );
        assert!(ctx.path("/etc/nftables.conf").is_file());
        assert!(ctx.path("/etc/tmux.conf").is_file());
    }

    #[test]
    fn setup_kernel_aborts_purge_on_empty_uname_or_failed_grub_and_skips_non_ii() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());

        // Case 1: empty uname -r must NOT purge any kernel
        let r_empty = MockCommandRunner::new();
        r_empty.on_exact("uname", &["-r"], ok_stdout(""));
        r_empty.on_exact("uname", &["-m"], ok_stdout("x86_64\n"));
        r_empty.on_prog("apt-get", ok_stdout(""));
        r_empty.on_exact("update-grub", &[], ok_stdout(""));
        r_empty.on_exact(
            "dpkg",
            &["-l"],
            ok_stdout("ii  linux-image-6.1.0-21-amd64  6.1.90-1  amd64\n"),
        );
        assert!(setup_kernel(&ctx, Some(true), &r_empty).unwrap());
        assert!(
            !r_empty
                .env_invocations()
                .iter()
                .any(|(p, a, _)| p == "apt-get" && a.first().map(String::as_str) == Some("purge"))
        );

        // Case 2: failed update-grub must NOT purge any kernel
        let r_grub = MockCommandRunner::new();
        r_grub.on_exact("uname", &["-r"], ok_stdout("6.1.0-21-amd64\n"));
        r_grub.on_exact("uname", &["-m"], ok_stdout("x86_64\n"));
        r_grub.on_prog("apt-get", ok_stdout(""));
        r_grub.on_exact("update-grub", &[], err_out());
        r_grub.on_exact(
            "dpkg",
            &["-l"],
            ok_stdout("ii  linux-image-6.1.0-18-amd64  6.1.76-1  amd64\n"),
        );
        assert!(setup_kernel(&ctx, Some(true), &r_grub).unwrap());
        assert!(
            !r_grub
                .env_invocations()
                .iter()
                .any(|(p, a, _)| p == "apt-get" && a.first().map(String::as_str) == Some("purge"))
        );

        // Case 3: non-ii (`rc`) package is skipped, only `ii` old kernel purged
        let r_rc = MockCommandRunner::new();
        r_rc.on_exact("uname", &["-r"], ok_stdout("6.1.0-21-amd64\n"));
        r_rc.on_exact("uname", &["-m"], ok_stdout("x86_64\n"));
        r_rc.on_prog("apt-get", ok_stdout(""));
        r_rc.on_exact("update-grub", &[], ok_stdout(""));
        r_rc.on_exact(
            "dpkg",
            &["-l"],
            ok_stdout(
                "rc  linux-image-6.1.0-17-amd64  6.1.70-1  amd64\n\
                 ii  linux-image-6.1.0-18-amd64  6.1.76-1  amd64\n\
                 ii  linux-image-6.1.0-21-amd64  6.1.90-1  amd64\n",
            ),
        );
        assert!(setup_kernel(&ctx, Some(true), &r_rc).unwrap());
        let purges: Vec<Vec<String>> = r_rc
            .env_invocations()
            .into_iter()
            .filter(|(p, a, _)| p == "apt-get" && a.first().map(String::as_str) == Some("purge"))
            .map(|(_, a, _)| a)
            .collect();
        assert_eq!(
            purges,
            vec![vec!["purge", "-yq", "linux-image-6.1.0-18-amd64"]]
        );
    }

    #[test]
    fn setup_memory_caps_swap_on_128gb_host_and_preserves_fstab_0644() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        fs::create_dir_all(ctx.path("/proc")).unwrap();
        fs::create_dir_all(ctx.path("/etc")).unwrap();
        // 128 GiB = 134217728 KiB -> capped at MAX_SWAP_MB (8192 MiB)
        fs::write(ctx.path("/proc/meminfo"), "MemTotal:      134217728 kB\n").unwrap();
        fs::write(ctx.path("/proc/swaps"), "Filename\tType\tSize\n").unwrap();
        let fstab = ctx.path("/etc/fstab");
        fs::write(&fstab, "UUID=root / ext4 defaults 0 1\n").unwrap();
        fs::set_permissions(&fstab, fs::Permissions::from_mode(0o644)).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("fallocate", ok_stdout(""));
        runner.on_prog("chmod", ok_stdout(""));
        runner.on_prog("mkswap", ok_stdout(""));
        runner.on_prog("swapon", ok_stdout(""));

        let rep = setup_memory(
            &ctx,
            &MemoryOptions {
                enable_zram: false,
                enable_swap: true,
            },
            &runner,
        )
        .unwrap();
        assert!(rep.swap_created);
        assert_eq!(rep.swap_size_mb, MAX_SWAP_MB);
        assert!(
            runner
                .invocations()
                .iter()
                .any(|(p, a)| p == "fallocate" && a.iter().any(|x| x == "8192M"))
        );
        assert_eq!(
            fs::metadata(&fstab).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // If fallocate and dd both fail, mkswap/swapon must NOT be invoked and swapfile cleaned
        let tmp2 = TempDir::new().unwrap();
        let ctx2 = SystemContext::with_root(tmp2.path());
        fs::create_dir_all(ctx2.path("/proc")).unwrap();
        fs::write(ctx2.path("/proc/meminfo"), "MemTotal:        524288 kB\n").unwrap();
        let r_fail = MockCommandRunner::new();
        r_fail.on_prog("fallocate", err_out());
        r_fail.on_prog("dd", err_out());
        let rep2 = setup_memory(
            &ctx2,
            &MemoryOptions {
                enable_zram: false,
                enable_swap: true,
            },
            &r_fail,
        )
        .unwrap();
        assert!(!rep2.swap_created);
        assert!(!r_fail.invocations().iter().any(|(p, _)| p == "mkswap"));
    }

    #[test]
    fn setup_logrotate_replaces_weekly_with_daily_and_preserves_0644() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        let conf = ctx.path("/etc/logrotate.conf");
        fs::create_dir_all(conf.parent().unwrap()).unwrap();
        fs::write(
            &conf,
            "# see man logrotate\nweekly\nrotate 4\n#compress\ninclude /etc/logrotate.d\n",
        )
        .unwrap();
        fs::set_permissions(&conf, fs::Permissions::from_mode(0o644)).unwrap();

        setup_logrotate(&ctx).unwrap();
        let lr = fs::read_to_string(&conf).unwrap();
        assert!(!lr.contains("weekly"), "weekly must be replaced: {lr}");
        assert_eq!(lr.lines().filter(|l| l.trim() == "daily").count(), 1);
        let daily_pos = lr.find("daily").unwrap();
        let include_pos = lr.find("include /etc/logrotate.d").unwrap();
        assert!(daily_pos < include_pos);
        assert!(lr.contains("rotate 7"));
        assert!(lr.contains("compress"));
        assert_eq!(
            fs::metadata(&conf).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn toggle_ip_forwarding_off_resets_ipv6() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        let fwd = ctx.path("/etc/sysctl.d/99-debopti-forwarding.conf");
        fs::create_dir_all(fwd.parent().unwrap()).unwrap();
        fs::write(&fwd, "net.ipv4.ip_forward = 1\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("sysctl", &["-n", "net.ipv4.ip_forward"], ok_stdout("1\n"));
        runner.on_prog("sysctl", ok_stdout(""));

        assert!(!toggle_ip_forwarding(&ctx, &runner).unwrap());
        assert!(!fwd.exists());
        let calls = runner.invocations();
        for expected in [
            "net.ipv4.ip_forward=0",
            "net.ipv4.conf.all.forwarding=0",
            "net.ipv4.conf.default.forwarding=0",
            "net.ipv6.conf.all.forwarding=0",
            "net.ipv6.conf.default.forwarding=0",
        ] {
            assert!(
                calls
                    .iter()
                    .any(|(p, a)| p == "sysctl"
                        && a == &vec!["-w".to_string(), expected.to_string()]),
                "missing sysctl reset for {expected}"
            );
        }
    }

    #[test]
    fn setup_base_handles_trixie_sid_deb822_and_propagates_apt_error() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SystemContext::with_root(tmp.path());
        ctx.is_cn_region = true;
        ctx.no_sys = false;
        fs::create_dir_all(ctx.path("/etc/apt/sources.list.d")).unwrap();
        fs::write(ctx.path("/etc/debian_version"), "trixie/sid\n").unwrap();
        let deb822 = ctx.path("/etc/apt/sources.list.d/debian.sources");
        fs::write(
            &deb822,
            "Types: deb\nURIs: http://deb.debian.org/debian\nSuites: trixie\nComponents: main\n",
        )
        .unwrap();
        fs::set_permissions(&deb822, fs::Permissions::from_mode(0o644)).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "ca-certificates"],
            ok_stdout("install ok installed"),
        );
        runner.on_prog("dpkg-query", err_out());
        runner.on_prog("apt-get", ok_stdout(""));

        let installed = setup_base(&ctx, &runner).unwrap();
        assert!(installed.contains(&"bind9-dnsutils".to_string()));
        let rewritten = fs::read_to_string(&deb822).unwrap();
        assert!(rewritten.contains("https://mirrors.tuna.tsinghua.edu.cn/debian"));
        assert!(!rewritten.contains("http://"));
        assert_eq!(
            fs::metadata(&deb822).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // APT install failure must propagate Err (not silently succeed)
        let runner_err = MockCommandRunner::new();
        runner_err.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "ca-certificates"],
            ok_stdout("install ok installed"),
        );
        runner_err.on_prog("dpkg-query", err_out());
        runner_err.on_prog("apt-get", err_out());
        assert!(setup_base(&ctx, &runner_err).is_err());
    }

    #[test]
    fn setup_limits_ensures_systemd_manager_header_and_preserves_0644() {
        let tmp = TempDir::new().unwrap();
        let ctx = SystemContext::with_root(tmp.path());
        setup_limits(&ctx).unwrap();

        for rel in ["/etc/systemd/system.conf", "/etc/systemd/user.conf"] {
            let p = ctx.path(rel);
            let txt = fs::read_to_string(&p).unwrap();
            assert!(
                txt.starts_with("[Manager]\n"),
                "{rel} missing [Manager]: {txt}"
            );
            assert!(txt.contains("DefaultLimitNOFILE=1048576"));
            assert_eq!(
                fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o644
            );
        }
    }
}
