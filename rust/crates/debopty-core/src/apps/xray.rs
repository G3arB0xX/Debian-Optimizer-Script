use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, DEFAULT_USER_AGENT, HttpClient, HttpRequest, IpPreference};
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "xray";

const DIAG_TARGET: &str = "debopty::apps::xray";
const XRAY_INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/XTLS/Xray-install/main/install-release.sh";
const XRAY_ASSET_DIR_ABS: &str = "/usr/local/share/xray";
const XRAY_CRON_SCRIPT_ABS: &str = "/usr/local/bin/xray-rule-update.sh";
const MIN_DAT_BYTES: u64 = 102_400;

const LOYALSOLDIER_GEOSITE_URL: &str =
    "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geosite.dat";
const LOYALSOLDIER_GEOIP_URL: &str =
    "https://github.com/Loyalsoldier/v2ray-rules-dat/releases/latest/download/geoip.dat";
const OFFICIAL_GEOSITE_URL: &str =
    "https://github.com/v2fly/domain-list-community/releases/latest/download/dlc.dat";
const OFFICIAL_GEOIP_URL: &str =
    "https://github.com/v2fly/geoip/releases/latest/download/geoip.dat";

/// CN mirrors supporting both GitHub API and Release endpoints (excluding `ghfast.top`).
const CN_MIRROR_CANDIDATES: &[&str] = &[
    "https://gh-proxy.com",
    "https://edgeone.gh-proxy.org",
    "https://gh-proxy.org",
];

fn app_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: MODULE_ID.to_string(),
        message: message.into(),
    }
}

fn strip_mirror_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"https?://[^/"' ]+/(https://(?:api\.github\.com/repos/XTLS/|github\.com/XTLS/Xray-(?:core|rules-dat)/releases/))"#,
        )
        .expect("valid strip mirror prefix regex")
    })
}

/// Active Xray geodata ruleset variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum XrayRuleset {
    #[default]
    Official,
    Loyalsoldier,
}

impl XrayRuleset {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Official => "official",
            Self::Loyalsoldier => "loyalsoldier",
        }
    }

    pub fn opposite(&self) -> Self {
        match self {
            Self::Official => Self::Loyalsoldier,
            Self::Loyalsoldier => Self::Official,
        }
    }
}

impl fmt::Display for XrayRuleset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for XrayRuleset {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "" | "official" => Ok(Self::Official),
            "loyalsoldier" => Ok(Self::Loyalsoldier),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid Xray ruleset '{other}': expected official|loyalsoldier"
            ))),
        }
    }
}

/// Snapshot of Xray Core installation, service, ruleset, and cron status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrayStatus {
    pub installed: bool,
    pub active: bool,
    pub ruleset: XrayRuleset,
    pub geo_auto_update: bool,
    pub cron_present: bool,
}

/// Probe CN mirrors in priority order, verifying both API (`"tag_name"`) and Release reachability.
pub fn select_cn_mirror(http: &dyn HttpClient) -> String {
    for &mirror in CN_MIRROR_CANDIDATES {
        let api_url =
            format!("{mirror}/https://api.github.com/repos/XTLS/Xray-core/releases/latest");
        let rel_url = format!("{mirror}/https://github.com/XTLS/Xray-core/releases/latest");

        let api_req = HttpRequest {
            url: &api_url,
            user_agent: Some(DEFAULT_USER_AGENT),
            connect_timeout: Duration::from_secs(5),
            max_timeout: Duration::from_secs(10),
            ip_preference: IpPreference::Any,
        };
        let rel_req = HttpRequest {
            url: &rel_url,
            user_agent: Some(DEFAULT_USER_AGENT),
            connect_timeout: Duration::from_secs(5),
            max_timeout: Duration::from_secs(10),
            ip_preference: IpPreference::Any,
        };

        let api_ok = http
            .get_text(&api_req)
            .is_ok_and(|r| (200..=299).contains(&r.status) && r.body.contains("\"tag_name\""));
        if !api_ok {
            continue;
        }
        let rel_ok = http
            .get_text(&rel_req)
            .is_ok_and(|r| (200..=299).contains(&r.status));
        if rel_ok {
            return mirror.to_string();
        }
    }
    CN_MIRROR_CANDIDATES[0].to_string()
}

/// Pure-Rust idempotent port of `_xray_rewrite_install_script_mirrors`.
///
/// Strips any prior mirror prefix before applying `mirror_base`, rewriting:
/// - `https://api.github.com/repos/XTLS/`
/// - `https://github.com/XTLS/Xray-rules-dat/releases/`
/// - `https://github.com/XTLS/Xray-core/releases/` EXCEPT lines containing `url_zip=`
///   so `--beta` JSON matching stays intact.
pub fn rewrite_install_script_mirrors(content: &str, mirror_base: &str) -> Result<String> {
    let base = mirror_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(CoreError::ValidationFailed(
            "mirror_base must not be empty".to_string(),
        ));
    }

    let strip_re = strip_mirror_prefix_re();
    let api_target = format!("{base}/https://api.github.com/repos/XTLS/");
    let rules_target = format!("{base}/https://github.com/XTLS/Xray-rules-dat/releases/");
    let core_target = format!("{base}/https://github.com/XTLS/Xray-core/releases/");

    let had_trailing_newline = content.ends_with('\n');
    let mut out_lines: Vec<String> = Vec::new();
    for line in content.lines() {
        let stripped = strip_re.replace_all(line, "$1");
        let step1 = stripped.replace("https://api.github.com/repos/XTLS/", &api_target);
        let step2 = step1.replace(
            "https://github.com/XTLS/Xray-rules-dat/releases/",
            &rules_target,
        );
        let final_line = if step2.contains("url_zip=") {
            step2
        } else {
            step2.replace("https://github.com/XTLS/Xray-core/releases/", &core_target)
        };
        out_lines.push(final_line);
    }

    let mut rendered = out_lines.join("\n");
    if had_trailing_newline {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn read_configured_ruleset(ctx: &AppContext) -> XrayRuleset {
    ctx.config_store()
        .get("XRAY_RULESET")
        .ok()
        .flatten()
        .and_then(|v| XrayRuleset::from_str(&v).ok())
        .unwrap_or(XrayRuleset::Official)
}

fn read_crontab_text(ctx: &AppContext, runner: &dyn CommandRunner) -> String {
    if ctx.no_sys {
        return String::new();
    }
    runner
        .run("crontab", &["-l"], Duration::from_secs(10))
        .ok()
        .filter(|o| o.success())
        .map(|o| o.stdout)
        .unwrap_or_default()
}

/// Determine the persisted or inferred `XRAY_GEO_AUTO_UPDATE` preference.
pub fn get_xray_geo_auto_update(ctx: &AppContext, runner: &dyn CommandRunner) -> bool {
    if let Ok(Some(val)) = ctx.config_store().get("XRAY_GEO_AUTO_UPDATE") {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            return trimmed == "true";
        }
    }

    let current_cron = read_crontab_text(ctx, runner);
    if current_cron.contains(XRAY_CRON_SCRIPT_ABS) {
        return true;
    }

    read_configured_ruleset(ctx) == XrayRuleset::Loyalsoldier
}

fn atomic_symlink_swap(target_rel: &str, link_path: &Path) -> Result<()> {
    let parent = link_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let file_name = link_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("link");
    let tmp_link = parent.join(format!(".{file_name}.debopty.tmp.{}", std::process::id()));
    if fs::symlink_metadata(&tmp_link).is_ok() {
        let _ = fs::remove_file(&tmp_link);
    }
    symlink(target_rel, &tmp_link)?;
    fs::rename(&tmp_link, link_path)?;
    Ok(())
}

fn fetch_dat_file(
    url: &str,
    dest: &Path,
    label: &str,
    is_cn: bool,
    http: &dyn HttpClient,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_file = dest.with_extension("dat.new");

    if let Err(first_err) = network::download_with_fallback(&tmp_file, url, is_cn, http) {
        if !is_cn {
            diag::log_event(
                LogLevel::Warn,
                DIAG_TARGET,
                None,
                "direct download of Xray geodata failed; retrying via CN mirror pool",
                None,
                &[("label", label)],
            );
            network::download_with_fallback(&tmp_file, url, true, http).map_err(|e| {
                let _ = fs::remove_file(&tmp_file);
                app_err(format!("failed to download {label}: {e}"))
            })?;
        } else {
            let _ = fs::remove_file(&tmp_file);
            return Err(app_err(format!("failed to download {label}: {first_err}")));
        }
    }

    let file_size = fs::metadata(&tmp_file).map(|m| m.len()).unwrap_or(0);
    if file_size > MIN_DAT_BYTES {
        fs::rename(&tmp_file, dest)?;
        Ok(())
    } else {
        let _ = fs::remove_file(&tmp_file);
        Err(app_err(format!(
            "{label} validation failed: file size {file_size} <= {MIN_DAT_BYTES} bytes"
        )))
    }
}

fn download_official_files(ctx: &AppContext, http: &dyn HttpClient) -> Result<()> {
    let asset_dir = ctx.path(XRAY_ASSET_DIR_ABS);
    fs::create_dir_all(&asset_dir)?;
    let is_cn = ctx.effective_is_cn();

    fetch_dat_file(
        OFFICIAL_GEOSITE_URL,
        &asset_dir.join("geosite.dat.official"),
        "official geosite.dat",
        is_cn,
        http,
    )?;
    fetch_dat_file(
        OFFICIAL_GEOIP_URL,
        &asset_dir.join("geoip.dat.official"),
        "official geoip.dat",
        is_cn,
        http,
    )?;
    Ok(())
}

fn download_loyalsoldier_files(ctx: &AppContext, http: &dyn HttpClient) -> Result<()> {
    let asset_dir = ctx.path(XRAY_ASSET_DIR_ABS);
    fs::create_dir_all(&asset_dir)?;
    let is_cn = ctx.effective_is_cn();

    fetch_dat_file(
        LOYALSOLDIER_GEOSITE_URL,
        &asset_dir.join("geosite.dat.loyalsoldier"),
        "Loyalsoldier geosite.dat",
        is_cn,
        http,
    )?;
    fetch_dat_file(
        LOYALSOLDIER_GEOIP_URL,
        &asset_dir.join("geoip.dat.loyalsoldier"),
        "Loyalsoldier geoip.dat",
        is_cn,
        http,
    )?;
    Ok(())
}

/// Render `/usr/local/bin/xray-rule-update.sh` (`0755`) and idempotently add its cron entry
/// while preserving 100% of existing user crontab entries (`U8`).
pub fn setup_xray_cron_job(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<PathBuf> {
    let cron_script = ctx.path(XRAY_CRON_SCRIPT_ABS);
    let mut vars = BTreeMap::new();
    vars.insert(
        "IS_CN".to_string(),
        if ctx.effective_is_cn() {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );
    ctx.template_engine().render_to_file(
        "templates/apps/xray/xray-rule-update.sh",
        &cron_script,
        &vars,
        Some(0o755),
    )?;

    if !ctx.no_sys {
        let current = read_crontab_text(ctx, runner);
        if !current.contains(XRAY_CRON_SCRIPT_ABS) {
            let mut lines: Vec<&str> = current.lines().collect();
            let entry = format!("30 3 * * 1 {XRAY_CRON_SCRIPT_ABS} >/dev/null 2>&1");
            lines.push(&entry);
            let mut new_cron = lines.join("\n");
            new_cron.push('\n');
            let _ = runner.run_with_stdin("crontab", &["-"], &new_cron, Duration::from_secs(10));
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "configured Xray geodata auto-update cron job",
        None,
        &[("script", &cron_script.to_string_lossy())],
    );
    Ok(cron_script)
}

/// Remove `/usr/local/bin/xray-rule-update.sh` and strip ONLY its line from crontab (`U8`).
pub fn cleanup_xray_geodata_cron(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<()> {
    let cron_script = ctx.path(XRAY_CRON_SCRIPT_ABS);
    if cron_script.exists() {
        let _ = fs::remove_file(&cron_script);
    }

    if !ctx.no_sys {
        let current = read_crontab_text(ctx, runner);
        if current.contains(XRAY_CRON_SCRIPT_ABS) {
            let filtered_lines: Vec<&str> = current
                .lines()
                .filter(|line| !line.contains(XRAY_CRON_SCRIPT_ABS))
                .collect();
            let mut new_cron = filtered_lines.join("\n");
            if !new_cron.is_empty() {
                new_cron.push('\n');
            }
            let _ = runner.run_with_stdin("crontab", &["-"], &new_cron, Duration::from_secs(10));
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "cleaned up Xray geodata auto-update cron job",
        None,
        &[],
    );
    Ok(())
}

/// Install or upgrade Xray Core (`--beta`), preserving `XRAY_RULESET` & `XRAY_GEO_AUTO_UPDATE`.
pub fn install_xray(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting Xray Core installation (--beta)",
        None,
        &[],
    );

    // Snapshot geo auto-update preference before installation.
    let geo_auto_update = get_xray_geo_auto_update(ctx, runner);
    let store = ctx.config_store();
    if store.path().is_file() {
        let has_key = store
            .get("XRAY_GEO_AUTO_UPDATE")
            .ok()
            .flatten()
            .is_some_and(|v| !v.trim().is_empty());
        if !has_key {
            let _ = store.set(
                "XRAY_GEO_AUTO_UPDATE",
                if geo_auto_update { "true" } else { "false" },
            );
        }
    }

    let is_cn = ctx.effective_is_cn();
    let tmp_script = NamedTempFile::new()?;
    let script_path = tmp_script.path().to_path_buf();
    network::download_with_fallback(&script_path, XRAY_INSTALL_SCRIPT_URL, is_cn, http)
        .map_err(|e| app_err(format!("failed to download Xray install script: {e}")))?;

    let mut used_mirror: Option<String> = None;
    if !ctx.no_sys {
        let script_str = script_path.to_string_lossy().into_owned();
        if is_cn {
            let primary = select_cn_mirror(http);
            let mut pool: Vec<String> = vec![primary.clone()];
            for &m in CN_MIRROR_CANDIDATES {
                if m != primary {
                    pool.push(m.to_string());
                }
            }

            let raw_script = fs::read_to_string(&script_path)?;
            let mut installed = false;
            for mirror in &pool {
                diag::log_event(
                    LogLevel::Info,
                    DIAG_TARGET,
                    None,
                    "rewriting Xray install script with CN mirror",
                    None,
                    &[("mirror", mirror)],
                );
                let rewritten = rewrite_install_script_mirrors(&raw_script, mirror)?;
                sys::write_file_atomic(&script_path, rewritten.as_bytes(), Some(0o755))?;
                let out = runner.run(
                    "bash",
                    &[&script_str, "install", "--beta"],
                    Duration::from_secs(300),
                )?;
                if out.success() {
                    installed = true;
                    used_mirror = Some(mirror.clone());
                    break;
                }
                diag::log_event(
                    LogLevel::Warn,
                    DIAG_TARGET,
                    None,
                    "Xray install failed via CN mirror; trying next candidate",
                    None,
                    &[("mirror", mirror)],
                );
            }
            if !installed {
                return Err(app_err(
                    "Xray Core installation failed across all CN mirrors",
                ));
            }
        } else {
            let out = runner.run(
                "bash",
                &[&script_str, "install", "--beta"],
                Duration::from_secs(300),
            )?;
            if !out.success() {
                return Err(app_err(format!(
                    "Xray Core installation failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                )));
            }
        }

        // Disable default autostart per 10 §6.
        let _ = runner.run("systemctl", &["disable", "xray"], Duration::from_secs(30));
    }

    // Backup regular geodata files to `.official` and self-heal if missing.
    let asset_dir = ctx.path(XRAY_ASSET_DIR_ABS);
    fs::create_dir_all(&asset_dir)?;
    for name in ["geosite.dat", "geoip.dat"] {
        let p = asset_dir.join(name);
        if let Ok(meta) = fs::symlink_metadata(&p)
            && meta.is_file()
        {
            fs::rename(&p, asset_dir.join(format!("{name}.official")))?;
        }
    }

    let mut warnings: Vec<String> = Vec::new();
    if (!asset_dir.join("geosite.dat.official").is_file()
        || !asset_dir.join("geoip.dat.official").is_file())
        && let Err(e) = download_official_files(ctx, http)
    {
        warnings.push(format!("official geodata self-heal skipped: {e}"));
    }

    let current_ruleset = read_configured_ruleset(ctx);
    match current_ruleset {
        XrayRuleset::Loyalsoldier => {
            if (!asset_dir.join("geosite.dat.loyalsoldier").is_file()
                || !asset_dir.join("geoip.dat.loyalsoldier").is_file())
                && let Err(e) = download_loyalsoldier_files(ctx, http)
            {
                warnings.push(format!("Loyalsoldier geodata self-heal skipped: {e}"));
            }
            atomic_symlink_swap("geosite.dat.loyalsoldier", &asset_dir.join("geosite.dat"))?;
            atomic_symlink_swap("geoip.dat.loyalsoldier", &asset_dir.join("geoip.dat"))?;
        }
        XrayRuleset::Official => {
            atomic_symlink_swap("geosite.dat.official", &asset_dir.join("geosite.dat"))?;
            atomic_symlink_swap("geoip.dat.official", &asset_dir.join("geoip.dat"))?;
        }
    }

    if geo_auto_update {
        setup_xray_cron_job(ctx, runner)?;
    } else {
        cleanup_xray_geodata_cron(ctx, runner)?;
    }

    let override_path = shared::inject_service_override(
        ctx,
        "xray",
        "templates/apps/xray/xray.service.override.conf",
        &BTreeMap::new(),
        runner,
    )
    .map_err(|e| app_err(format!("failed to inject xray systemd override: {e}")))?;

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        format!("Xray Core deployed (autostart disabled, ruleset: {current_ruleset})"),
    )
    .with_detail("ruleset", current_ruleset.as_str())
    .with_detail(
        "geo_auto_update",
        if geo_auto_update { "true" } else { "false" },
    )
    .with_detail(
        "override_path",
        override_path.to_string_lossy().into_owned(),
    )
    .with_preserved(format!(
        "preserved XRAY_GEO_AUTO_UPDATE={geo_auto_update} and XRAY_RULESET={current_ruleset}"
    ));

    if let Some(m) = used_mirror {
        report = report.with_detail("cn_mirror", m);
    }
    for w in warnings {
        report = report.with_warning(w);
    }

    Ok(report)
}

/// Toggle between `official` and `loyalsoldier` geodata rulesets atomically without touching cron.
pub fn toggle_xray_ruleset(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let asset_dir = ctx.path(XRAY_ASSET_DIR_ABS);
    fs::create_dir_all(&asset_dir)?;

    let current = read_configured_ruleset(ctx);
    let target = current.opposite();

    match target {
        XrayRuleset::Loyalsoldier => {
            if !asset_dir.join("geosite.dat.loyalsoldier").is_file()
                || !asset_dir.join("geoip.dat.loyalsoldier").is_file()
            {
                download_loyalsoldier_files(ctx, http)?;
            }
            atomic_symlink_swap("geosite.dat.loyalsoldier", &asset_dir.join("geosite.dat"))?;
            atomic_symlink_swap("geoip.dat.loyalsoldier", &asset_dir.join("geoip.dat"))?;
        }
        XrayRuleset::Official => {
            if !asset_dir.join("geosite.dat.official").is_file()
                || !asset_dir.join("geoip.dat.official").is_file()
            {
                download_official_files(ctx, http)?;
            }
            atomic_symlink_swap("geosite.dat.official", &asset_dir.join("geosite.dat"))?;
            atomic_symlink_swap("geoip.dat.official", &asset_dir.join("geoip.dat"))?;
        }
    }

    ctx.config_store().set("XRAY_RULESET", target.as_str())?;

    let mut restarted = false;
    if !ctx.no_sys {
        let active = runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "xray"],
                Duration::from_secs(10),
            )
            .map(|o| o.success())
            .unwrap_or(false);
        if active {
            let _ = runner.run("systemctl", &["restart", "xray"], Duration::from_secs(30));
            restarted = true;
        }
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "switched Xray ruleset",
        None,
        &[
            ("from", current.as_str()),
            ("to", target.as_str()),
            ("restarted", if restarted { "true" } else { "false" }),
        ],
    );

    Ok(AppReport::new(
        MODULE_ID,
        "toggle-ruleset",
        true,
        format!("switched Xray ruleset from {current} to {target}"),
    )
    .with_detail("previous_ruleset", current.as_str())
    .with_detail("ruleset", target.as_str())
    .with_detail("restarted", if restarted { "true" } else { "false" }))
}

/// Toggle the Xray geodata weekly auto-update cron job independently of ruleset selection.
pub fn toggle_xray_cron(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    let currently_enabled = get_xray_geo_auto_update(ctx, runner);
    if currently_enabled {
        cleanup_xray_geodata_cron(ctx, runner)?;
        ctx.config_store().set("XRAY_GEO_AUTO_UPDATE", "false")?;
        Ok(AppReport::new(
            MODULE_ID,
            "toggle-cron",
            true,
            "disabled Xray geodata auto-update cron job",
        )
        .with_detail("geo_auto_update", "false"))
    } else {
        let script_path = setup_xray_cron_job(ctx, runner)?;
        ctx.config_store().set("XRAY_GEO_AUTO_UPDATE", "true")?;
        Ok(AppReport::new(
            MODULE_ID,
            "toggle-cron",
            true,
            "enabled Xray geodata auto-update cron job (Mondays 03:30)",
        )
        .with_detail("geo_auto_update", "true")
        .with_detail("script_path", script_path.to_string_lossy().into_owned()))
    }
}

/// Thoroughly uninstall Xray Core, systemd units, overrides, assets, cron job, and config keys.
pub fn uninstall_xray(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Xray Core",
        None,
        &[],
    );

    if !ctx.no_sys {
        let tmp_script = NamedTempFile::new()?;
        let script_path = tmp_script.path().to_path_buf();
        if network::download_with_fallback(
            &script_path,
            XRAY_INSTALL_SCRIPT_URL,
            ctx.effective_is_cn(),
            http,
        )
        .is_ok()
        {
            let script_str = script_path.to_string_lossy().into_owned();
            let _ = runner.run("bash", &[&script_str, "remove"], Duration::from_secs(120));
        }

        let _ = runner.run("systemctl", &["stop", "xray"], Duration::from_secs(30));
        let _ = runner.run("systemctl", &["disable", "xray"], Duration::from_secs(30));
    }

    let systemd_dir = ctx.path("/etc/systemd/system");
    if let Ok(entries) = fs::read_dir(&systemd_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with("xray")
            {
                let p = entry.path();
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
            }
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    cleanup_xray_geodata_cron(ctx, runner)?;

    for abs in [
        "/usr/bin/xray",
        "/usr/local/bin/xray",
        "/usr/local/etc/xray",
        "/etc/xray",
        "/opt/xray",
        "/etc/systemd/system/xray.service.d",
        XRAY_ASSET_DIR_ABS,
    ] {
        let p = ctx.path(abs);
        if let Ok(meta) = fs::symlink_metadata(&p) {
            if meta.is_dir() {
                let _ = fs::remove_dir_all(&p);
            } else {
                let _ = fs::remove_file(&p);
            }
        }
    }

    let store = ctx.config_store();
    let _ = store.set("XRAY_RULESET", "");
    let _ = store.set("XRAY_GEO_AUTO_UPDATE", "");

    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "Xray Core, geodata rulesets, and cron job removed",
    ))
}

/// Probe current Xray installation, service activity, ruleset, and cron status.
pub fn probe_xray_status(ctx: &AppContext, runner: &dyn CommandRunner) -> XrayStatus {
    let installed = ctx.is_cmd_present("xray", runner);
    let active = !ctx.no_sys
        && runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "xray"],
                Duration::from_secs(10),
            )
            .map(|o| o.success())
            .unwrap_or(false);
    let ruleset = read_configured_ruleset(ctx);
    let geo_auto_update = get_xray_geo_auto_update(ctx, runner);
    let cron_present = read_crontab_text(ctx, runner).contains(XRAY_CRON_SCRIPT_ABS);

    XrayStatus {
        installed,
        active,
        ruleset,
        geo_auto_update,
        cron_present,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrite_install_script_mirrors_is_idempotent_and_preserves_url_zip() {
        let sample = concat!(
            "API='https://api.github.com/repos/XTLS/Xray-core/releases'\n",
            "RULES='https://github.com/XTLS/Xray-rules-dat/releases/latest/download/geoip.dat'\n",
            "DOWNLOAD='https://github.com/XTLS/Xray-core/releases/download/v1.0/Xray.zip'\n",
            "url_zip=\"https://github.com/XTLS/Xray-core/releases/download/${VER}/Xray-linux-64.zip\"\n",
        );

        let pass1 = rewrite_install_script_mirrors(sample, "https://gh-proxy.com/").unwrap();
        assert!(pass1.contains("https://gh-proxy.com/https://api.github.com/repos/XTLS/"));
        assert!(
            pass1.contains("https://gh-proxy.com/https://github.com/XTLS/Xray-rules-dat/releases/")
        );
        assert!(pass1.contains(
            "DOWNLOAD='https://gh-proxy.com/https://github.com/XTLS/Xray-core/releases/download/"
        ));
        assert!(
            pass1.contains(
                "url_zip=\"https://github.com/XTLS/Xray-core/releases/download/${VER}/Xray-linux-64.zip\""
            )
        );

        // Idempotent on second pass with same mirror
        let pass2 = rewrite_install_script_mirrors(&pass1, "https://gh-proxy.com").unwrap();
        assert_eq!(pass1, pass2);

        // Cleanly switches to another mirror without stacking prefixes
        let pass3 = rewrite_install_script_mirrors(&pass2, "https://edgeone.gh-proxy.org").unwrap();
        assert!(!pass3.contains("gh-proxy.com"));
        assert!(pass3.contains("https://edgeone.gh-proxy.org/https://api.github.com/repos/XTLS/"));
        assert!(
            pass3.contains(
                "url_zip=\"https://github.com/XTLS/Xray-core/releases/download/${VER}/Xray-linux-64.zip\""
            )
        );
    }
}
