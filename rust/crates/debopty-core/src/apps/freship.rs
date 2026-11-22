use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::{NamedTempFile, TempDir};

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, DEFAULT_USER_AGENT, HttpClient, HttpRequest, IpPreference};
use crate::sys::{self, CommandRunner};

pub const MODULE_ID: &str = "freship";
pub const FRESHIP_REPO_RAW: &str = "https://raw.githubusercontent.com/hotyue/IP-Sentinel/main";

const DIAG_TARGET: &str = "debopty::apps::freship";
const FRESHIP_OPT_DIR_ABS: &str = "/opt/freship";
const FRESHIP_CONF_DIR_ABS: &str = "/etc/freship";
const FRESHIP_CONF_FILE_ABS: &str = "/etc/freship/freship.conf";
const FRESHIP_MAP_CACHE_ABS: &str = "/opt/freship/data/map.json";

const CORE_SCRIPTS: &[&str] = &[
    "freship_lib.sh",
    "freship_probe.sh",
    "freship_mod_google.sh",
    "freship_mod_trust.sh",
    "freship_runner.sh",
    "freship_updater.sh",
    "freship_core.sh",
];

fn app_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: MODULE_ID.to_string(),
        message: message.into(),
    }
}

fn journal_iso_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})\S*\s+(.*)$")
            .expect("valid journal iso regex")
    })
}

fn journal_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^freship(?:\[\d+\])?:\s*(.*)$").expect("valid journal prefix regex")
    })
}

fn journal_date_strip_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^([^|]+)\s\|\s\d{4}-\d{2}-\d{2}\s\|(.*)$")
            .expect("valid journal date strip regex")
    })
}

/// FreshIP network stack execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshipWorkMode {
    Ipv4Only,
    Ipv6Only,
    DualStack,
}

impl FreshipWorkMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ipv4Only => "ipv4_only",
            Self::Ipv6Only => "ipv6_only",
            Self::DualStack => "dual_stack",
        }
    }

    pub fn enables_v4(&self) -> bool {
        matches!(self, Self::Ipv4Only | Self::DualStack)
    }

    pub fn enables_v6(&self) -> bool {
        matches!(self, Self::Ipv6Only | Self::DualStack)
    }
}

impl fmt::Display for FreshipWorkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for FreshipWorkMode {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim() {
            "ipv4_only" => Ok(Self::Ipv4Only),
            "ipv6_only" => Ok(Self::Ipv6Only),
            "dual_stack" => Ok(Self::DualStack),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid FreshIP work_mode '{other}': expected ipv4_only|ipv6_only|dual_stack"
            ))),
        }
    }
}

/// Top-level IP-Sentinel `map.json` schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipMap {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub continents: Vec<FreshipContinent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipContinent {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub countries: Vec<FreshipCountry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipCountry {
    pub id: String,
    pub name: String,
    pub keyword_file: String,
    #[serde(default)]
    pub states: Vec<FreshipState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipState {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub cities: Vec<FreshipCity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipCity {
    pub id: String,
    pub name: String,
}

/// Explicit non-interactive target selection for FreshIP installation or reconfiguration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipSelection {
    pub country_id: String,
    pub state_id: String,
    pub city_id: String,
    pub city_name: String,
    pub kw_filename: String,
    pub work_mode: Option<FreshipWorkMode>,
    pub bind_ipv4: Option<String>,
    pub bind_ipv6: Option<String>,
}

fn validate_freship_segment(raw: &str, label: &str) -> Result<()> {
    if raw.is_empty()
        || raw.starts_with('-')
        || raw.contains("..")
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid FreshIP {label}: '{raw}'"
        )));
    }
    Ok(())
}

impl FreshipSelection {
    /// Strictly validate all path segments, display name, and optional bind IPs (BE-P2-13).
    pub fn validate(&self) -> Result<()> {
        validate_freship_segment(&self.country_id, "country_id")?;
        validate_freship_segment(&self.state_id, "state_id")?;
        validate_freship_segment(&self.city_id, "city_id")?;
        validate_freship_segment(&self.kw_filename, "kw_filename")?;

        if self.city_name.trim().is_empty()
            || self
                .city_name
                .contains(['\r', '\n', '\0', '"', '\'', '$', '`', ';', '\\', '|', '&'])
        {
            return Err(CoreError::ValidationFailed(format!(
                "invalid FreshIP city_name: '{}'",
                self.city_name
            )));
        }

        if let Some(ref v4) = self.bind_ipv4
            && !v4.trim().is_empty()
            && v4.trim().parse::<Ipv4Addr>().is_err()
        {
            return Err(CoreError::ValidationFailed(format!(
                "invalid FreshIP bind_ipv4 address: '{v4}'"
            )));
        }

        if let Some(ref v6) = self.bind_ipv6
            && !v6.trim().is_empty()
            && v6.trim().parse::<Ipv6Addr>().is_err()
        {
            return Err(CoreError::ValidationFailed(format!(
                "invalid FreshIP bind_ipv6 address: '{v6}'"
            )));
        }

        Ok(())
    }

    pub fn remote_path(&self) -> String {
        format!("{}/{}/{}", self.country_id, self.state_id, self.city_id)
    }

    /// Select the first available country/state/city entry from a parsed `FreshipMap`.
    pub fn from_map_default(map: &FreshipMap) -> Result<Self> {
        for continent in &map.continents {
            for country in &continent.countries {
                for state in &country.states {
                    if let Some(city) = state.cities.first() {
                        return Ok(Self {
                            country_id: country.id.clone(),
                            state_id: state.id.clone(),
                            city_id: city.id.clone(),
                            city_name: city.name.clone(),
                            kw_filename: country.keyword_file.clone(),
                            work_mode: None,
                            bind_ipv4: None,
                            bind_ipv6: None,
                        });
                    }
                }
            }
        }
        Err(app_err("FreshIP map.json contains no selectable cities"))
    }
}

/// Per-instance (`v4`/`v6`) probe state read from `/etc/freship/state/{inst}.state`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipInstanceState {
    pub run_mode: String,
    pub last_score: String,
}

/// Aggregate FreshIP installation, configuration, timer, and probe status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshipStatus {
    pub installed: bool,
    pub region_code: String,
    pub region_name: String,
    pub work_mode: Option<FreshipWorkMode>,
    pub v4_timer_active: bool,
    pub v6_timer_active: bool,
    pub updater_timer_active: bool,
    pub v4_state: Option<FreshipInstanceState>,
    pub v6_state: Option<FreshipInstanceState>,
}

/// Pure-Rust port of `_get_utc_offset`.
pub fn utc_offset_for_country(code: &str) -> &'static str {
    match code.trim() {
        "JP" | "KR" => "+9",
        "SG" | "HK" | "TW" | "MY" | "PH" | "CN" => "+8",
        "VN" | "TH" | "ID" => "+7",
        "UK" | "IE" | "PT" => "+0",
        "FR" | "DE" | "ES" | "IT" | "NL" | "BE" | "CH" | "PL" | "SE" | "NO" | "DK" => "+1",
        "TR" | "SA" | "RU" | "UA" | "GR" | "RO" => "+3",
        "US" | "CA" => "-5",
        "AU" => "+10",
        "NG" | "ZA" => "+1",
        _ => "+0",
    }
}

/// Pure-Rust port of `_freship_target_cc` (strips `-*` suffix and maps `UK -> GB`).
pub fn target_cc_for_country(code: &str) -> String {
    let base = code.trim().split('-').next().unwrap_or("");
    if base == "UK" {
        "GB".to_string()
    } else {
        base.to_string()
    }
}

/// Parse IP-Sentinel `map.json` strictly via `serde_json`.
pub fn parse_freship_map(json_str: &str) -> Result<FreshipMap> {
    let map: FreshipMap = serde_json::from_str(json_str)
        .map_err(|e| app_err(format!("failed to parse FreshIP map.json: {e}")))?;
    if map.continents.is_empty() {
        return Err(app_err("FreshIP map.json has no continents"));
    }
    Ok(map)
}

/// Fetch `map.json` from upstream with U8 local offline cache fallback at `/opt/freship/data/map.json`.
///
/// Returns `(FreshipMap, used_offline_cache)`.
pub fn fetch_freship_map(ctx: &AppContext, http: &dyn HttpClient) -> Result<(FreshipMap, bool)> {
    let cache_path = ctx.path(FRESHIP_MAP_CACHE_ABS);
    let remote_url = format!("{FRESHIP_REPO_RAW}/data/map.json");
    let tmp_map = NamedTempFile::new()?;
    let tmp_path = tmp_map.path().to_path_buf();

    match network::download_with_fallback(&tmp_path, &remote_url, ctx.effective_is_cn(), http) {
        Ok(_) => {
            let raw = fs::read_to_string(&tmp_path)?;
            let map = parse_freship_map(&raw)?;
            if let Some(parent) = cache_path.parent() {
                fs::create_dir_all(parent)?;
            }
            sys::write_file_atomic(&cache_path, raw.as_bytes(), Some(0o644))?;
            Ok((map, false))
        }
        Err(remote_err) => {
            if cache_path.is_file() {
                diag::log_event(
                    LogLevel::Warn,
                    DIAG_TARGET,
                    None,
                    "remote FreshIP map fetch failed; falling back to local offline cache",
                    None,
                    &[("cache", &cache_path.to_string_lossy())],
                );
                let raw = fs::read_to_string(&cache_path)?;
                let map = parse_freship_map(&raw)?;
                Ok((map, true))
            } else {
                Err(app_err(format!(
                    "failed to fetch FreshIP map.json and no local cache exists: {remote_err}"
                )))
            }
        }
    }
}

fn parse_global_ipv6_addrs(ip_output: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in ip_output.lines() {
        if !line.contains("inet6") || line.contains("temporary") || line.contains("deprecated") {
            continue;
        }
        let Some(after) = line.split("inet6 ").nth(1) else {
            continue;
        };
        let Some(addr) = after.split('/').next() else {
            continue;
        };
        let addr = addr.trim();
        let lower = addr.to_ascii_lowercase();
        if addr.is_empty() || lower.starts_with("::1") || lower.starts_with("fe80") {
            continue;
        }
        if addr.chars().all(|c| c.is_ascii_hexdigit() || c == ':') {
            out.push(addr.to_string());
        }
    }
    out
}

/// Detect public outbound IPv4 and IPv6 addresses (dual-track IPv6 detection).
pub fn detect_public_ips(
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> (Option<String>, Option<String>) {
    let mut detect_v4: Option<String> = None;
    for url in ["https://api.ip.sb/ip", "https://ifconfig.me"] {
        let req = HttpRequest {
            url,
            user_agent: Some(DEFAULT_USER_AGENT),
            connect_timeout: Duration::from_secs(5),
            max_timeout: Duration::from_secs(5),
            ip_preference: IpPreference::Ipv4Only,
        };
        if let Ok(resp) = http.get_text(&req)
            && (200..=299).contains(&resp.status)
        {
            let cleaned: String = resp.body.chars().filter(|c| !c.is_whitespace()).collect();
            if !cleaned.is_empty() && cleaned.contains('.') {
                detect_v4 = Some(cleaned);
                break;
            }
        }
    }

    let mut detect_v6: Option<String> = None;
    if let Ok(ip_out) = runner.run(
        "ip",
        &["-6", "addr", "show", "scope", "global"],
        Duration::from_secs(10),
    ) && ip_out.success()
    {
        for v6 in parse_global_ipv6_addrs(&ip_out.stdout) {
            let curl_res = runner.run(
                "curl",
                &[
                    "-6",
                    "-s",
                    "-m",
                    "6",
                    "-o",
                    "/dev/null",
                    "--interface",
                    &v6,
                    "https://ipv6.google.com",
                ],
                Duration::from_secs(10),
            );
            if curl_res.is_ok_and(|o| o.success()) {
                detect_v6 = Some(v6);
                break;
            }
        }
    }

    if detect_v6.is_none() {
        for url in ["https://api.ip.sb/ip", "https://icanhazip.com"] {
            let req = HttpRequest {
                url,
                user_agent: Some(DEFAULT_USER_AGENT),
                connect_timeout: Duration::from_secs(5),
                max_timeout: Duration::from_secs(8),
                ip_preference: IpPreference::Ipv6Only,
            };
            if let Ok(resp) = http.get_text(&req)
                && (200..=299).contains(&resp.status)
            {
                let cleaned: String = resp.body.chars().filter(|c| !c.is_whitespace()).collect();
                if cleaned.contains(':') {
                    detect_v6 = Some(cleaned);
                    break;
                }
            }
        }
    }

    (detect_v4, detect_v6)
}

fn resolve_work_mode_and_binds(
    sel: &FreshipSelection,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<(FreshipWorkMode, String, String)> {
    if let Some(mode) = sel.work_mode {
        let mut b4 = sel.bind_ipv4.clone().unwrap_or_default();
        let mut b6 = sel.bind_ipv6.clone().unwrap_or_default();
        if (mode.enables_v4() && b4.is_empty()) || (mode.enables_v6() && b6.is_empty()) {
            let (d4, d6) = detect_public_ips(http, runner);
            if mode.enables_v4() && b4.is_empty() {
                b4 = d4.unwrap_or_default();
            }
            if mode.enables_v6() && b6.is_empty() {
                b6 = d6.unwrap_or_default();
            }
        }
        return Ok((mode, b4, b6));
    }

    let (d4, d6) = detect_public_ips(http, runner);
    let b4 = sel.bind_ipv4.clone().or(d4).unwrap_or_default();
    let b6 = sel.bind_ipv6.clone().or(d6).unwrap_or_default();

    match (!b4.is_empty(), !b6.is_empty()) {
        (true, true) => Ok((FreshipWorkMode::DualStack, b4, b6)),
        (true, false) => Ok((FreshipWorkMode::Ipv4Only, b4, String::new())),
        (false, true) => Ok((FreshipWorkMode::Ipv6Only, String::new(), b6)),
        (false, false) => Err(app_err(
            "no public IPv4 or IPv6 egress address detected for FreshIP",
        )),
    }
}

fn deploy_core_scripts_into(ctx: &AppContext, target_opt_dir: &Path) -> Result<()> {
    let core_dir = target_opt_dir.join("core");
    fs::create_dir_all(&core_dir)?;
    fs::create_dir_all(target_opt_dir.join("logs"))?;
    fs::create_dir_all(target_opt_dir.join("data/cookies"))?;

    let mut vars = BTreeMap::new();
    vars.insert("INSTALL_DIR".to_string(), FRESHIP_OPT_DIR_ABS.to_string());
    let engine = ctx.template_engine();

    for &script in CORE_SCRIPTS {
        let tmpl = format!("templates/apps/freship/{script}");
        let dest = core_dir.join(script);
        engine.render_to_file(&tmpl, &dest, &vars, Some(0o755))?;
    }
    Ok(())
}

fn pull_freship_assets_into(
    ctx: &AppContext,
    target_opt_dir: &Path,
    remote_path: &str,
    kw_filename: &str,
    cached_map_bytes: Option<&[u8]>,
    http: &dyn HttpClient,
) -> Result<()> {
    let is_cn = ctx.effective_is_cn();
    let region_dest = target_opt_dir.join(format!("data/regions/{remote_path}.json"));
    if let Some(parent) = region_dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir_all(target_opt_dir.join("data/keywords"))?;

    network::download_with_fallback(
        &region_dest,
        &format!("{FRESHIP_REPO_RAW}/data/regions/{remote_path}.json"),
        is_cn,
        http,
    )
    .map_err(|e| {
        app_err(format!(
            "failed to download FreshIP region data '{remote_path}': {e}"
        ))
    })?;

    network::download_with_fallback(
        &target_opt_dir.join(format!("data/keywords/{kw_filename}")),
        &format!("{FRESHIP_REPO_RAW}/data/keywords/{kw_filename}"),
        is_cn,
        http,
    )
    .map_err(|e| {
        app_err(format!(
            "failed to download FreshIP keyword file '{kw_filename}': {e}"
        ))
    })?;

    network::download_with_fallback(
        &target_opt_dir.join("data/user_agents.txt"),
        &format!("{FRESHIP_REPO_RAW}/data/user_agents.txt"),
        is_cn,
        http,
    )
    .map_err(|e| app_err(format!("failed to download FreshIP user_agents.txt: {e}")))?;

    let map_dest = target_opt_dir.join("data/map.json");
    if let Err(map_err) = network::download_with_fallback(
        &map_dest,
        &format!("{FRESHIP_REPO_RAW}/data/map.json"),
        is_cn,
        http,
    ) {
        if let Some(cached) = cached_map_bytes {
            sys::write_file_atomic(&map_dest, cached, Some(0o644))?;
        } else {
            return Err(app_err(format!(
                "failed to download FreshIP map.json: {map_err}"
            )));
        }
    }
    Ok(())
}

fn render_freship_conf(
    ctx: &AppContext,
    sel: &FreshipSelection,
    work_mode: FreshipWorkMode,
    bind_v4: &str,
    bind_v6: &str,
) -> Result<()> {
    sel.validate()?;
    let conf_file = ctx.path(FRESHIP_CONF_FILE_ABS);
    let mut vars = BTreeMap::new();
    vars.insert("REGION_CODE".to_string(), sel.country_id.clone());
    vars.insert("REGION_NAME".to_string(), sel.city_name.clone());
    vars.insert("REGION_PATH".to_string(), sel.remote_path());
    vars.insert("KW_FILE".to_string(), sel.kw_filename.clone());
    vars.insert(
        "TARGET_CC".to_string(),
        target_cc_for_country(&sel.country_id),
    );
    vars.insert(
        "UTC_OFFSET".to_string(),
        utc_offset_for_country(&sel.country_id).to_string(),
    );
    vars.insert("BIND_IPV4".to_string(), bind_v4.to_string());
    vars.insert("BIND_IPV6".to_string(), bind_v6.to_string());
    vars.insert("WORK_MODE".to_string(), work_mode.as_str().to_string());
    vars.insert("FRESHIP_DIR".to_string(), FRESHIP_OPT_DIR_ABS.to_string());

    ctx.template_engine().render_to_file(
        "templates/apps/freship/freship.conf",
        &conf_file,
        &vars,
        Some(0o600),
    )?;
    Ok(())
}

fn clear_freship_state(ctx: &AppContext) {
    for inst in ["v4", "v6"] {
        let p = ctx.path(&format!("/etc/freship/state/{inst}.state"));
        if p.exists() {
            let _ = fs::remove_file(&p);
        }
    }
}

fn deploy_freship_systemd(
    ctx: &AppContext,
    mode: FreshipWorkMode,
    runner: &dyn CommandRunner,
) -> Result<()> {
    let engine = ctx.template_engine();
    let empty = BTreeMap::new();
    for unit in [
        "freship-core@.service",
        "freship-core@.timer",
        "freship-updater.service",
        "freship-updater.timer",
    ] {
        engine.render_to_file(
            &format!("templates/apps/freship/{unit}"),
            &ctx.path(&format!("/etc/systemd/system/{unit}")),
            &empty,
            Some(0o644),
        )?;
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
        let _ = runner.run(
            "systemctl",
            &["enable", "--now", "freship-updater.timer"],
            Duration::from_secs(30),
        );
        if mode.enables_v4() {
            let _ = runner.run(
                "systemctl",
                &["enable", "--now", "freship-core@v4.timer"],
                Duration::from_secs(30),
            );
        }
        if mode.enables_v6() {
            let _ = runner.run(
                "systemctl",
                &["enable", "--now", "freship-core@v6.timer"],
                Duration::from_secs(30),
            );
        }
    }
    Ok(())
}

/// Install FreshIP v4 engine, scripts, config (`0600`), and systemd timers.
pub fn install_freship(
    ctx: &AppContext,
    selection: Option<FreshipSelection>,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "starting FreshIP v4 installation",
        None,
        &[],
    );

    if !ctx.no_sys {
        sys::safe_apt_install(
            &["curl", "jq", "unzip", "file", "coreutils", "less", "bc"],
            runner,
        )
        .map_err(|e| app_err(format!("failed to install FreshIP prerequisites: {e}")))?;
    }

    shared::create_system_user(ctx, "freship", runner)
        .map_err(|e| app_err(format!("failed to create freship user: {e}")))?;

    // Preserve cached map.json before staging so U8 offline fallback works on reinstall.
    let cached_map_bytes = fs::read(ctx.path(FRESHIP_MAP_CACHE_ABS)).ok();
    let (resolved_sel, used_offline_cache) = match selection {
        Some(sel) => (sel, false),
        None => {
            let (map, offline) = fetch_freship_map(ctx, http)?;
            (FreshipSelection::from_map_default(&map)?, offline)
        }
    };
    resolved_sel.validate()?;

    let (work_mode, bind_v4, bind_v6) = resolve_work_mode_and_binds(&resolved_sel, http, runner)?;

    // BE-P1-25: stage all FreshIP binaries, assets, and scripts into a sibling temp dir BEFORE touching `/opt/freship`,
    // and fail loudly with E4004 if mandatory downloads or extraction fail.
    let opt_dir = ctx.path(FRESHIP_OPT_DIR_ABS);
    let opt_parent = opt_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("/opt"));
    fs::create_dir_all(opt_parent)?;

    let stage_guard = TempDir::new_in(opt_parent)?;
    let stage_root = stage_guard.path().to_path_buf();
    for sub in [
        "bin",
        "core",
        "data/keywords",
        "data/regions",
        "data/cookies",
        "logs",
    ] {
        fs::create_dir_all(stage_root.join(sub))?;
    }

    let pkg_arch = if ctx.arch() == "aarch64" || ctx.arch() == "arm64" {
        "aarch64-linux-gnu"
    } else {
        "x86_64-linux-gnu"
    };
    let dl_url = format!(
        "https://github.com/lwthiker/curl-impersonate/releases/download/v0.6.1/curl-impersonate-v0.6.1.{pkg_arch}.tar.gz"
    );
    let tmp_tar = NamedTempFile::new()?;
    let tmp_tar_path = tmp_tar.path().to_path_buf();
    network::download_with_fallback(&tmp_tar_path, &dl_url, ctx.effective_is_cn(), http)
        .map_err(|e| app_err(format!("failed to download curl-impersonate archive: {e}")))?;

    let bin_dir = stage_root.join("bin");
    if !ctx.no_sys {
        shared::extract_tar_gz(&tmp_tar_path, &bin_dir, runner)
            .map_err(|e| app_err(format!("failed to extract curl-impersonate archive: {e}")))?;
        if let Ok(entries) = fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(0o755));
                }
            }
        }
    }

    pull_freship_assets_into(
        ctx,
        &stage_root,
        &resolved_sel.remote_path(),
        &resolved_sel.kw_filename,
        cached_map_bytes.as_deref(),
        http,
    )?;
    deploy_core_scripts_into(ctx, &stage_root)?;

    // Atomic directory swap into `/opt/freship` only after all downloads & staging succeed
    let stage_path = stage_guard.keep();
    let backup_dir = opt_parent.join(format!(".freship.old.{}", std::process::id()));
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(&backup_dir);
    }
    let had_old = opt_dir.exists();
    if had_old {
        fs::rename(&opt_dir, &backup_dir)?;
    }
    if let Err(swap_err) = fs::rename(&stage_path, &opt_dir) {
        if had_old && backup_dir.exists() {
            let _ = fs::rename(&backup_dir, &opt_dir);
        }
        let _ = fs::remove_dir_all(&stage_path);
        return Err(CoreError::Io(swap_err));
    }
    if had_old && backup_dir.exists() {
        let _ = fs::remove_dir_all(&backup_dir);
    }

    let conf_dir = ctx.path(FRESHIP_CONF_DIR_ABS);
    fs::create_dir_all(conf_dir.join("state"))?;
    render_freship_conf(ctx, &resolved_sel, work_mode, &bind_v4, &bind_v6)?;

    if !ctx.no_sys && !ctx.is_custom_root() {
        shared::best_effort_chown(ctx, "freship", &conf_dir, true, runner);
        shared::best_effort_chown(ctx, "freship", &opt_dir, true, runner);
    }

    clear_freship_state(ctx);
    deploy_freship_systemd(ctx, work_mode, runner)?;

    let mut report = AppReport::new(
        MODULE_ID,
        "install",
        true,
        format!(
            "FreshIP v4 deployed ({}/{}, mode={work_mode})",
            resolved_sel.country_id, resolved_sel.city_name
        ),
    )
    .with_detail("region_code", &resolved_sel.country_id)
    .with_detail("region_name", &resolved_sel.city_name)
    .with_detail("work_mode", work_mode.as_str())
    .with_detail(
        "utc_offset",
        utc_offset_for_country(&resolved_sel.country_id),
    )
    .with_detail("target_cc", target_cc_for_country(&resolved_sel.country_id));

    if used_offline_cache {
        report = report.with_warning("used local offline map.json cache");
    }

    Ok(report)
}

/// Reconfigure FreshIP region/mode, reset `v4.state`/`v6.state`, and re-enable matching timers.
pub fn reconfigure_freship(
    ctx: &AppContext,
    selection: Option<FreshipSelection>,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "reconfiguring FreshIP v4",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &[
                "disable",
                "--now",
                "freship-core@v4.timer",
                "freship-core@v6.timer",
            ],
            Duration::from_secs(30),
        );
    }

    let (resolved_sel, used_offline_cache) = match selection {
        Some(sel) => (sel, false),
        None => {
            let (map, offline) = fetch_freship_map(ctx, http)?;
            (FreshipSelection::from_map_default(&map)?, offline)
        }
    };
    resolved_sel.validate()?;

    let cached_map_bytes = fs::read(ctx.path(FRESHIP_MAP_CACHE_ABS)).ok();
    let opt_dir = ctx.path(FRESHIP_OPT_DIR_ABS);
    let (work_mode, bind_v4, bind_v6) = resolve_work_mode_and_binds(&resolved_sel, http, runner)?;
    pull_freship_assets_into(
        ctx,
        &opt_dir,
        &resolved_sel.remote_path(),
        &resolved_sel.kw_filename,
        cached_map_bytes.as_deref(),
        http,
    )?;
    deploy_core_scripts_into(ctx, &opt_dir)?;
    render_freship_conf(ctx, &resolved_sel, work_mode, &bind_v4, &bind_v6)?;
    clear_freship_state(ctx);

    if !ctx.no_sys && !ctx.is_custom_root() {
        shared::best_effort_chown(
            ctx,
            "freship",
            &ctx.path(FRESHIP_CONF_DIR_ABS),
            true,
            runner,
        );
        shared::best_effort_chown(ctx, "freship", &opt_dir, true, runner);
    }

    deploy_freship_systemd(ctx, work_mode, runner)?;

    let mut report = AppReport::new(
        MODULE_ID,
        "reconfigure",
        true,
        format!(
            "FreshIP reconfigured ({}/{}, mode={work_mode}); instance states reset",
            resolved_sel.country_id, resolved_sel.city_name
        ),
    )
    .with_detail("region_code", &resolved_sel.country_id)
    .with_detail("region_name", &resolved_sel.city_name)
    .with_detail("work_mode", work_mode.as_str());

    if used_offline_cache {
        report = report.with_warning("used local offline map.json cache");
    }

    Ok(report)
}

/// Enable or disable FreshIP v4/v6 timers according to `WORK_MODE` in `freship.conf`.
pub fn toggle_freship_timers(
    ctx: &AppContext,
    enable: Option<bool>,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let status = probe_freship_status(ctx, runner);
    let currently_active = status.v4_timer_active || status.v6_timer_active;
    let should_enable = enable.unwrap_or(!currently_active);
    let mode = status.work_mode.unwrap_or(FreshipWorkMode::DualStack);

    if !ctx.no_sys {
        if should_enable {
            if mode.enables_v4() {
                let _ = runner.run(
                    "systemctl",
                    &["enable", "--now", "freship-core@v4.timer"],
                    Duration::from_secs(30),
                );
            }
            if mode.enables_v6() {
                let _ = runner.run(
                    "systemctl",
                    &["enable", "--now", "freship-core@v6.timer"],
                    Duration::from_secs(30),
                );
            }
            let _ = runner.run(
                "systemctl",
                &["enable", "--now", "freship-updater.timer"],
                Duration::from_secs(30),
            );
        } else {
            let _ = runner.run(
                "systemctl",
                &[
                    "disable",
                    "--now",
                    "freship-core@v4.timer",
                    "freship-core@v6.timer",
                ],
                Duration::from_secs(30),
            );
        }
    }

    Ok(AppReport::new(
        MODULE_ID,
        "toggle-timers",
        true,
        if should_enable {
            "FreshIP timers enabled"
        } else {
            "FreshIP timers suspended"
        },
    )
    .with_detail("enabled", if should_enable { "true" } else { "false" })
    .with_detail("work_mode", mode.as_str()))
}

/// Run `/opt/freship/core/freship_updater.sh` as user `freship` to refresh hot data.
pub fn sync_freship_data(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    let updater = ctx.path("/opt/freship/core/freship_updater.sh");
    let is_exec = updater.is_file()
        && fs::metadata(&updater)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    if !is_exec {
        return Err(app_err(
            "FreshIP is not installed or freship_updater.sh is missing/non-executable",
        ));
    }

    if !ctx.no_sys {
        let updater_str = updater.to_string_lossy().into_owned();
        let out = shared::run_as_user(
            ctx,
            "freship",
            "bash",
            &[&updater_str],
            &[],
            Duration::from_secs(120),
            runner,
        )?;
        if !out.success() {
            return Err(app_err(format!(
                "freship_updater.sh failed (code {}): {}",
                out.code,
                out.stderr.trim()
            )));
        }
    }

    Ok(AppReport::new(
        MODULE_ID,
        "sync-data",
        true,
        "FreshIP hot data synchronized",
    ))
}

/// Uninstall FreshIP timers, systemd units, `/opt/freship`, `/etc/freship`, and `freship` user.
pub fn uninstall_freship(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling FreshIP",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &[
                "disable",
                "--now",
                "freship-core@v4.timer",
                "freship-core@v6.timer",
                "freship-updater.timer",
            ],
            Duration::from_secs(30),
        );
    }

    for unit in [
        "freship-core@.service",
        "freship-core@.timer",
        "freship-updater.service",
        "freship-updater.timer",
    ] {
        let p = ctx.path(&format!("/etc/systemd/system/{unit}"));
        if p.exists() {
            let _ = fs::remove_file(&p);
        }
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    for dir in [FRESHIP_OPT_DIR_ABS, FRESHIP_CONF_DIR_ABS] {
        let p = ctx.path(dir);
        if p.exists() {
            let _ = fs::remove_dir_all(&p);
        }
    }

    let _ = shared::remove_system_user(ctx, "freship", false, runner)?;

    Ok(AppReport::new(
        MODULE_ID,
        "uninstall",
        true,
        "FreshIP uninstalled",
    ))
}

fn parse_simple_kv(path: &Path) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let Ok(text) = fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            map.insert(k.trim().to_string(), val);
        }
    }
    map
}

fn read_instance_state(ctx: &AppContext, inst: &str) -> Option<FreshipInstanceState> {
    let sf = ctx.path(&format!("/etc/freship/state/{inst}.state"));
    if !sf.is_file() {
        return None;
    }
    let kv = parse_simple_kv(&sf);
    let run_mode = kv.get("RUN_MODE").cloned().unwrap_or_default();
    let last_score = kv.get("LAST_SCORE").cloned().unwrap_or_default();
    if run_mode.is_empty() && last_score.is_empty() {
        None
    } else {
        Some(FreshipInstanceState {
            run_mode,
            last_score,
        })
    }
}

fn is_unit_active(ctx: &AppContext, unit: &str, runner: &dyn CommandRunner) -> bool {
    if ctx.no_sys {
        return false;
    }
    runner
        .run(
            "systemctl",
            &["is-active", "--quiet", unit],
            Duration::from_secs(10),
        )
        .map(|o| o.success())
        .unwrap_or(false)
}

/// Probe FreshIP configuration, per-stack state files, and systemd timer status.
pub fn probe_freship_status(ctx: &AppContext, runner: &dyn CommandRunner) -> FreshipStatus {
    let conf_path = ctx.path(FRESHIP_CONF_FILE_ABS);
    let installed = conf_path.is_file() && ctx.path("/opt/freship/core/freship_core.sh").is_file();
    let kv = parse_simple_kv(&conf_path);

    let region_code = kv.get("REGION_CODE").cloned().unwrap_or_default();
    let region_name = kv.get("REGION_NAME").cloned().unwrap_or_default();
    let work_mode = kv
        .get("WORK_MODE")
        .and_then(|v| FreshipWorkMode::from_str(v).ok());

    FreshipStatus {
        installed,
        region_code,
        region_name,
        work_mode,
        v4_timer_active: is_unit_active(ctx, "freship-core@v4.timer", runner),
        v6_timer_active: is_unit_active(ctx, "freship-core@v6.timer", runner),
        updater_timer_active: is_unit_active(ctx, "freship-updater.timer", runner),
        v4_state: read_instance_state(ctx, "v4"),
        v6_state: read_instance_state(ctx, "v6"),
    }
}

fn normalize_journal_msg(raw: &str) -> String {
    let mut msg = if let Some(caps) = journal_prefix_re().captures(raw) {
        caps.get(1).map_or(raw, |m| m.as_str()).to_string()
    } else {
        raw.to_string()
    };

    if let Some(stripped) = msg.strip_prefix("[FreshIP] ") {
        if let Some(caps) = journal_date_strip_re().captures(stripped) {
            let p1 = caps.get(1).map_or("", |m| m.as_str());
            let p2 = caps.get(2).map_or("", |m| m.as_str());
            msg = format!("{p1} |{p2}");
        } else {
            msg = stripped.to_string();
        }
    } else if let Some(stripped) = msg.strip_prefix("[FreshIP]") {
        msg = stripped.trim_start().to_string();
    }

    msg
}

/// Pure-Rust 1:1 port of `_freship_normalize_journal_msg` and `_freship_show_journal_logs`.
pub fn format_freship_journal_logs(raw_journal: &str) -> Vec<String> {
    let iso_re = journal_iso_re();
    let mut out = Vec::new();
    for line in raw_journal.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(caps) = iso_re.captures(trimmed) {
            let date = &caps[1];
            let time = &caps[2];
            let raw_msg = &caps[3];
            let msg = normalize_journal_msg(raw_msg);
            out.push(format!("{date} {time} [FreshIP] {msg}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_offset_and_target_cc_match_bash_table() {
        assert_eq!(utc_offset_for_country("JP"), "+9");
        assert_eq!(utc_offset_for_country("SG"), "+8");
        assert_eq!(utc_offset_for_country("US"), "-5");
        assert_eq!(utc_offset_for_country("AU"), "+10");
        assert_eq!(utc_offset_for_country("UK"), "+0");
        assert_eq!(utc_offset_for_country("ZZ"), "+0");

        assert_eq!(target_cc_for_country("UK"), "GB");
        assert_eq!(target_cc_for_country("UK-LDN"), "GB");
        assert_eq!(target_cc_for_country("US-CA"), "US");
        assert_eq!(target_cc_for_country("JP"), "JP");
    }

    #[test]
    fn format_freship_journal_logs_normalizes_iso_and_duplicate_dates() {
        let raw = concat!(
            "2026-11-21T08:15:30+0800 freship[1234]: [FreshIP] v4 | 2026-11-21 | probe=ok\n",
            "2026-11-21T08:16:00Z freship: [FreshIP] updater synced\n",
            "not-a-timestamp line ignored\n",
        );
        let formatted = format_freship_journal_logs(raw);
        assert_eq!(
            formatted,
            vec![
                "2026-11-21 08:15:30 [FreshIP] v4 | probe=ok",
                "2026-11-21 08:16:00 [FreshIP] updater synced",
            ]
        );
    }
}
