use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use walkdir::WalkDir;

use crate::apps::shared::{self, AppContext, AppReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::sys::{self, CommandRunner};

const DIAG_TARGET: &str = "debopty::apps::devops::lego";
const LEGO_FALLBACK_TAG: &str = "v5.2.2";
const LEGO_ENVS_DIR_ABS: &str = "/etc/lego/envs";
const LEGO_CERTS_DIR_ABS: &str = "/var/lib/lego/certificates";

fn default_provider() -> String {
    "cloudflare".to_string()
}

/// Domain enrollment request for `add_lego_domain`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegoDomainSpec {
    pub primary_domain: String,
    #[serde(default)]
    pub san_domains: Vec<String>,
    pub email: String,
    pub cf_token: String,
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub issue_immediately: bool,
}

/// Parsed status summary for a configured Lego domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegoDomainStatus {
    pub primary_domain: String,
    pub domains: String,
    pub email: String,
    pub provider: String,
    pub auto_renew: bool,
    pub ferron_push: bool,
    pub dns_skip_propagation: bool,
    pub env_path: PathBuf,
    pub cert_exists: bool,
    pub days_left: Option<i64>,
}

/// Toggleable boolean flags in a Lego domain `.env` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LegoFlag {
    AutoRenew,
    DnsSkipPropagation,
    FerronPush,
}

impl LegoFlag {
    fn env_key(self) -> &'static str {
        match self {
            Self::AutoRenew => "export DEBOPTI_AUTO_RENEW",
            Self::DnsSkipPropagation => "export DEBOPTI_DNS_SKIP_PROPAGATION",
            Self::FerronPush => "export DEBOPTI_FERRON_PUSH",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AutoRenew => "DEBOPTI_AUTO_RENEW",
            Self::DnsSkipPropagation => "DEBOPTI_DNS_SKIP_PROPAGATION",
            Self::FerronPush => "DEBOPTI_FERRON_PUSH",
        }
    }
}

fn lego_err(message: impl Into<String>) -> CoreError {
    CoreError::App {
        app: "lego".to_string(),
        message: message.into(),
    }
}

fn contains_forbidden_shell_or_path_chars(s: &str) -> bool {
    s.is_empty()
        || s.starts_with('-')
        || s.contains('/')
        || s.contains('\\')
        || s.contains("..")
        || s.contains('"')
        || s.contains('\'')
        || s.contains(';')
        || s.contains('$')
        || s.contains('`')
        || s.contains('|')
        || s.contains('&')
        || s.contains('\0')
        || s.chars().any(char::is_whitespace)
}

/// Validate a Lego DNS provider identifier (`[A-Za-z0-9_-]+`, no leading `-`, no shell metacharacters).
pub fn validate_provider(provider: &str) -> Result<()> {
    if provider.is_empty()
        || provider.starts_with('-')
        || !provider
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Lego DNS provider: '{provider}'"
        )));
    }
    Ok(())
}

/// Validate a primary domain (used as `.env` filename): rejects `*`, `/`, `..`, quotes, spaces, CR/LF, `;`, shell meta.
pub fn validate_primary_domain(domain: &str) -> Result<()> {
    if contains_forbidden_shell_or_path_chars(domain) || domain.contains('*') {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Lego primary domain: '{domain}'"
        )));
    }
    Ok(())
}

/// Validate a SAN domain entry: permits `*.example.com` wildcard prefix while rejecting path/shell injection chars.
pub fn validate_san_domain(domain: &str) -> Result<()> {
    if contains_forbidden_shell_or_path_chars(domain) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Lego SAN domain: '{domain}'"
        )));
    }
    if let Some(rest) = domain.strip_prefix("*.") {
        if rest.is_empty() || rest.contains('*') || rest.starts_with('.') {
            return Err(CoreError::ValidationFailed(format!(
                "invalid Lego wildcard SAN domain: '{domain}'"
            )));
        }
    } else if domain.contains('*') {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Lego SAN domain wildcard placement: '{domain}'"
        )));
    }
    Ok(())
}

/// Escape double quotes, backslashes, `$`, and backticks for safe double-quoted `.env` serialization.
pub fn escape_env_value(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

fn unescape_env_value(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    let mut chars = val.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                match next {
                    '\\' | '"' | '$' | '`' => out.push(next),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            } else {
                out.push('\\');
            }
        } else {
            out.push(ch);
        }
    }
    out
}

/// Pure-Rust `.env` parser (ZERO shell `source` execution!).
fn parse_lego_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    let text = fs::read_to_string(path)?;
    let mut map = BTreeMap::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let body = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((k, v_raw)) = body.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() {
            continue;
        }
        let v_trimmed = v_raw.trim();
        let value = if let Some(inner) = v_trimmed
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
        {
            unescape_env_value(inner)
        } else if let Some(inner) = v_trimmed
            .strip_prefix('\'')
            .and_then(|s| s.strip_suffix('\''))
        {
            inner.to_string()
        } else {
            v_trimmed.to_string()
        };
        map.insert(key.to_string(), value);
    }
    Ok(map)
}

fn is_ferron_installed(ctx: &AppContext, runner: &dyn CommandRunner) -> bool {
    ctx.path("/etc/ferron").is_dir() || ctx.is_cmd_present("ferron", runner)
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

fn find_file_named(root: &Path, filename: &str) -> Option<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .find(|e| e.file_type().is_file() && e.file_name().to_str() == Some(filename))
        .map(|e| e.into_path())
}

/// Convert Gregorian civil date (year, month 1..=12, day 1..=31) to Unix days since 1970-01-01.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = u64::from(month);
    let d = u64::from(day);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Parse OpenSSL `notAfter=May 20 12:00:00 2027 GMT` timestamp into Unix epoch seconds.
pub fn parse_openssl_enddate_epoch(enddate_line: &str) -> Option<i64> {
    let raw = enddate_line
        .trim()
        .strip_prefix("notAfter=")
        .unwrap_or(enddate_line.trim())
        .trim();
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if tokens.len() < 4 {
        return None;
    }
    let month: u32 = match tokens[0] {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day: u32 = tokens[1].parse().ok()?;
    let hms: Vec<&str> = tokens[2].split(':').collect();
    if hms.len() != 3 {
        return None;
    }
    let hour: i64 = hms[0].parse().ok()?;
    let min: i64 = hms[1].parse().ok()?;
    let sec: i64 = hms[2].parse().ok()?;
    let year: i64 = tokens[3].parse().ok()?;
    if !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&min)
        || !(0..61).contains(&sec)
    {
        return None;
    }

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + min * 60 + sec)
}

fn probe_cert_days_left(cert_path: &Path, runner: &dyn CommandRunner) -> Option<i64> {
    let cert_str = cert_path.to_string_lossy().into_owned();
    let out = runner
        .run(
            "openssl",
            &["x509", "-enddate", "-noout", "-in", &cert_str],
            Duration::from_secs(10),
        )
        .ok()
        .filter(|o| o.success())?;

    let end_epoch = parse_openssl_enddate_epoch(&out.stdout)?;
    let now_epoch = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    Some((end_epoch - now_epoch) / 86_400)
}

/// Deploy the Lego ACME client binary, automated renewal scripts, and systemd timer.
pub fn install_lego(
    ctx: &AppContext,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "installing Lego ACME certificate tool",
        None,
        &[],
    );

    let arch = ctx.arch();
    let lego_arch = match arch.as_str() {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => {
            return Err(lego_err(format!(
                "unsupported architecture for Lego: {arch}"
            )));
        }
    };

    let is_cn = ctx.effective_is_cn();
    let mut warnings = Vec::new();
    let mut version = match network::get_latest_github_release("go-acme/lego", is_cn, http)
        .ok()
        .flatten()
    {
        Some(v) if !v.trim().is_empty() => v,
        _ => {
            warnings.push(format!(
                "获取 Lego 最新版本失败，回退至稳定版 {LEGO_FALLBACK_TAG}"
            ));
            LEGO_FALLBACK_TAG.to_string()
        }
    };
    if !version.starts_with('v') {
        version = format!("v{version}");
    }

    let url = format!(
        "https://github.com/go-acme/lego/releases/download/{version}/lego_{version}_linux_{lego_arch}.tar.gz"
    );
    let tmp_dir = TempDir::new()?;
    let tmp_tar = tmp_dir.path().join("lego.tar.gz");
    let extract_dir = tmp_dir.path().join("extract");

    network::download_with_fallback(&tmp_tar, &url, is_cn, http)
        .map_err(|e| lego_err(format!("failed to download Lego archive: {e}")))?;
    shared::extract_tar_gz(&tmp_tar, &extract_dir, runner)
        .map_err(|e| lego_err(format!("failed to extract Lego archive: {e}")))?;

    let bin_lego = find_file_named(&extract_dir, "lego")
        .ok_or_else(|| lego_err("extracted Lego archive missing 'lego' binary"))?;
    let lego_dest = ctx.path("/usr/local/bin/lego");
    sys::write_file_atomic(&lego_dest, &fs::read(&bin_lego)?, Some(0o755))?;

    // Working directories (0700)
    for dir_abs in [
        LEGO_ENVS_DIR_ABS,
        "/var/lib/lego",
        "/var/lib/lego/accounts",
        LEGO_CERTS_DIR_ABS,
    ] {
        let p = ctx.path(dir_abs);
        fs::create_dir_all(&p)?;
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700))?;
    }

    let engine = ctx.template_engine();
    let empty_vars = BTreeMap::new();
    engine.render_to_file(
        "templates/apps/lego/debopti-lego-lib.sh",
        &ctx.path("/usr/local/bin/debopti-lego-lib.sh"),
        &empty_vars,
        Some(0o644),
    )?;
    for script in [
        "debopti-lego-run-once.sh",
        "debopti-lego-renew.sh",
        "debopti-lego-hook.sh",
    ] {
        engine.render_to_file(
            &format!("templates/apps/lego/{script}"),
            &ctx.path(&format!("/usr/local/bin/{script}")),
            &empty_vars,
            Some(0o755),
        )?;
    }

    engine.render_to_file(
        "templates/apps/lego/debopti-lego-renew.service",
        &ctx.path("/etc/systemd/system/debopti-lego-renew.service"),
        &empty_vars,
        Some(0o644),
    )?;
    engine.render_to_file(
        "templates/apps/lego/debopti-lego-renew.timer",
        &ctx.path("/etc/systemd/system/debopti-lego-renew.timer"),
        &empty_vars,
        Some(0o644),
    )?;

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
        let _ = runner.run(
            "systemctl",
            &["enable", "--now", "debopti-lego-renew.timer"],
            Duration::from_secs(30),
        );

        let lego_bin_cmd = if ctx.is_custom_root() {
            lego_dest.to_string_lossy().into_owned()
        } else {
            "/usr/local/bin/lego".to_string()
        };
        if runner
            .run(
                &lego_bin_cmd,
                &["migrate", "--help"],
                Duration::from_secs(10),
            )
            .map(|o| o.success())
            .unwrap_or(false)
        {
            let _ = runner.run(
                &lego_bin_cmd,
                &["migrate", "--path=/var/lib/lego"],
                Duration::from_secs(30),
            );
        }
    }

    if is_ferron_installed(ctx, runner) {
        let _ = shared::prepare_shared_certs_dir(ctx, runner)?;
    }

    let mut report = AppReport::new("lego", "install", true, "Lego certificate manager deployed")
        .with_detail("version", version);
    for w in warnings {
        report = report.with_warning(w);
    }
    Ok(report)
}

/// Uninstall Lego binary, renewal scripts, systemd timer/service, and `/etc/lego` + `/var/lib/lego`.
pub fn uninstall_lego(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<AppReport> {
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "uninstalling Lego certificate manager",
        None,
        &[],
    );

    if !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &["disable", "--now", "debopti-lego-renew.timer"],
            Duration::from_secs(30),
        );
        let _ = runner.run(
            "systemctl",
            &["stop", "debopti-lego-renew.service"],
            Duration::from_secs(30),
        );
    }

    for p in [
        "/usr/local/bin/lego",
        "/usr/local/bin/debopti-lego-lib.sh",
        "/usr/local/bin/debopti-lego-run-once.sh",
        "/usr/local/bin/debopti-lego-renew.sh",
        "/usr/local/bin/debopti-lego-hook.sh",
        "/etc/systemd/system/debopti-lego-renew.service",
        "/etc/systemd/system/debopti-lego-renew.timer",
    ] {
        remove_path_no_follow(&ctx.path(p));
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    remove_path_no_follow(&ctx.path("/etc/lego"));

    // BE-P2-13 (U8): preserve `/var/lib/lego` when certificates exist; only remove when empty
    let mut report = AppReport::new(
        "lego",
        "uninstall",
        true,
        "Lego certificate manager uninstalled",
    );
    let certs_dir = ctx.path(LEGO_CERTS_DIR_ABS);
    let has_certs = certs_dir.is_dir()
        && fs::read_dir(&certs_dir)
            .ok()
            .and_then(|mut rd| rd.next())
            .is_some();
    if has_certs {
        report = report.with_preserved(format!(
            "preserved existing TLS certificates under {LEGO_CERTS_DIR_ABS}"
        ));
    } else {
        remove_path_no_follow(&ctx.path("/var/lib/lego"));
    }

    Ok(report)
}

/// Enroll a new domain configuration at `/etc/lego/envs/{primary}.env` (`0600`).
/// Never logs or exposes `spec.cf_token` (`U10`).
pub fn add_lego_domain(
    ctx: &AppContext,
    spec: &LegoDomainSpec,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    let primary = spec.primary_domain.trim();
    validate_primary_domain(primary)?;

    let email = spec.email.trim();
    if email.is_empty() || !email.contains('@') || email.contains(['"', '\'', '\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Lego contact email: '{email}'"
        )));
    }

    if spec.cf_token.trim().is_empty() || spec.cf_token.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(
            "Lego Cloudflare API token must be non-empty without control characters".to_string(),
        ));
    }

    let provider = if spec.provider.trim().is_empty() {
        "cloudflare"
    } else {
        spec.provider.trim()
    };
    validate_provider(provider)?;

    let mut all_domains = vec![primary.to_string()];
    for san in &spec.san_domains {
        let trimmed = san.trim();
        if trimmed.is_empty() {
            continue;
        }
        validate_san_domain(trimmed)?;
        if trimmed != primary {
            all_domains.push(trimmed.to_string());
        }
    }
    let domains_csv = all_domains.join(",");

    let envs_dir = ctx.path(LEGO_ENVS_DIR_ABS);
    let env_file = envs_dir.join(format!("{primary}.env"));
    if env_file.exists() {
        return Err(lego_err(format!(
            "Lego domain '{primary}' already exists at {}",
            env_file.display()
        )));
    }

    fs::create_dir_all(&envs_dir)?;
    fs::set_permissions(&envs_dir, fs::Permissions::from_mode(0o700))?;

    let ferron_push = if is_ferron_installed(ctx, runner) {
        let _ = shared::prepare_shared_certs_dir(ctx, runner)?;
        "true"
    } else {
        "false"
    };

    let mut vars = BTreeMap::new();
    vars.insert(
        "CF_TOKEN".to_string(),
        escape_env_value(spec.cf_token.trim()),
    );
    vars.insert("DOMAINS".to_string(), escape_env_value(&domains_csv));
    vars.insert("EMAIL".to_string(), escape_env_value(email));
    vars.insert("PROVIDER".to_string(), escape_env_value(provider));
    vars.insert("AUTO_RENEW".to_string(), "true".to_string());
    vars.insert("FERRON_PUSH".to_string(), ferron_push.to_string());
    vars.insert("DNS_SKIP_PROPAGATION".to_string(), "false".to_string());

    ctx.template_engine().render_to_file(
        "templates/apps/lego/lego.env",
        &env_file,
        &vars,
        Some(0o600),
    )?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "added Lego domain configuration",
        None,
        &[
            ("primary_domain", primary),
            ("provider", provider),
            ("ferron_push", ferron_push),
        ],
    );

    let mut report = AppReport::new("lego", "add-domain", true, "Lego domain added")
        .with_detail("primary_domain", primary)
        .with_detail("domains", domains_csv)
        .with_detail("ferron_push", ferron_push)
        .with_detail("env_path", env_file.to_string_lossy().into_owned());

    if spec.issue_immediately {
        match run_lego_once(ctx, primary, "issue", runner) {
            Ok(_) => {
                report = report.with_detail("initial_issue", "ok");
            }
            Err(e) => {
                report = report
                    .with_detail("initial_issue", "failed")
                    .with_warning(format!("首次证书申请失败：{e}"));
            }
        }
    }

    Ok(report)
}

/// List all configured Lego domains by safely parsing `/etc/lego/envs/*.env` without shell `source`.
pub fn list_lego_domains(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<LegoDomainStatus>> {
    let envs_dir = ctx.path(LEGO_ENVS_DIR_ABS);
    if !envs_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut env_files: Vec<PathBuf> = fs::read_dir(&envs_dir)?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("env"))
        .collect();
    env_files.sort();

    let mut statuses = Vec::new();
    for env_path in env_files {
        let map = parse_lego_env_file(&env_path)?;
        let domains = map.get("DEBOPTI_DOMAINS").cloned().unwrap_or_default();
        let primary_domain = domains
            .split(',')
            .map(str::trim)
            .find(|d| validate_primary_domain(d).is_ok())
            .map(ToString::to_string)
            .or_else(|| {
                env_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(ToString::to_string)
            })
            .unwrap_or_default();

        let cert_path = ctx.path(&format!("{LEGO_CERTS_DIR_ABS}/{primary_domain}.crt"));
        let cert_exists = cert_path.is_file();
        let days_left = if cert_exists {
            probe_cert_days_left(&cert_path, runner)
        } else {
            None
        };

        statuses.push(LegoDomainStatus {
            primary_domain,
            domains,
            email: map.get("DEBOPTI_EMAIL").cloned().unwrap_or_default(),
            provider: map
                .get("DEBOPTI_PROVIDER")
                .cloned()
                .unwrap_or_else(default_provider),
            auto_renew: map.get("DEBOPTI_AUTO_RENEW").map(String::as_str) == Some("true"),
            ferron_push: map.get("DEBOPTI_FERRON_PUSH").map(String::as_str) == Some("true"),
            dns_skip_propagation: map.get("DEBOPTI_DNS_SKIP_PROPAGATION").map(String::as_str)
                == Some("true"),
            env_path,
            cert_exists,
            days_left,
        });
    }

    Ok(statuses)
}

/// Toggle `AutoRenew`, `DnsSkipPropagation`, or `FerronPush` in `/etc/lego/envs/{primary}.env`
/// using atomic `set_conf_value` while preserving `0600` permissions.
pub fn toggle_lego_flag(
    ctx: &AppContext,
    primary_domain: &str,
    flag: LegoFlag,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    validate_primary_domain(primary_domain)?;
    let env_file = ctx.path(&format!("{LEGO_ENVS_DIR_ABS}/{primary_domain}.env"));
    if !env_file.is_file() {
        return Err(lego_err(format!(
            "Lego env file not found for '{primary_domain}'"
        )));
    }

    let map = parse_lego_env_file(&env_file)?;
    let current = map.get(flag.label()).map(String::as_str) == Some("true");
    let next = !current;
    let next_str = if next { "true" } else { "false" };

    sys::set_conf_value(&env_file, flag.env_key(), &format!("\"{next_str}\""), "=")?;
    fs::set_permissions(&env_file, fs::Permissions::from_mode(0o600))?;

    let mut report = AppReport::new("lego", "toggle-flag", true, "Lego domain flag toggled")
        .with_detail("primary_domain", primary_domain)
        .with_detail("flag", flag.label())
        .with_detail("value", next_str);

    if flag == LegoFlag::FerronPush && next {
        let cert_path = ctx.path(&format!("{LEGO_CERTS_DIR_ABS}/{primary_domain}.crt"));
        if cert_path.is_file() {
            match run_lego_ferron_hook(ctx, primary_domain, runner) {
                Ok(_) => {
                    report = report.with_detail("ferron_hook_triggered", "true");
                }
                Err(e) => {
                    report = report
                        .with_detail("ferron_hook_triggered", "failed")
                        .with_warning(format!("推送证书至 Ferron 失败：{e}"));
                }
            }
        }
    }

    Ok(report)
}

/// Execute `/usr/local/bin/debopti-lego-run-once.sh` for a configured domain (`issue|renew|auto|run`).
pub fn run_lego_once(
    ctx: &AppContext,
    primary_domain: &str,
    mode: &str,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    validate_primary_domain(primary_domain)?;
    // BE-P1-09: accept CLI alias "run" -> "auto" alongside issue|renew|auto
    let normalized_mode = match mode.trim() {
        "issue" => "issue",
        "renew" => "renew",
        "auto" | "run" => "auto",
        other => {
            return Err(CoreError::ValidationFailed(format!(
                "invalid Lego run mode '{other}': expected issue|renew|auto|run"
            )));
        }
    };

    let env_file = ctx.path(&format!("{LEGO_ENVS_DIR_ABS}/{primary_domain}.env"));
    if !env_file.is_file() {
        return Err(lego_err(format!(
            "Lego env file not found for '{primary_domain}'"
        )));
    }

    let script = ctx.path("/usr/local/bin/debopti-lego-run-once.sh");
    if !script.is_file() {
        return Err(lego_err(
            "debopti-lego-run-once.sh not found; reinstall Lego first",
        ));
    }

    if !ctx.no_sys {
        let script_str = script.to_string_lossy().into_owned();
        let env_str = env_file.to_string_lossy().into_owned();
        let interactive = if ctx.non_interactive { "0" } else { "1" };
        let out = runner.run_with_env(
            &script_str,
            &[&env_str, normalized_mode],
            &[("DEBOPTI_INTERACTIVE_LEG", interactive)],
            Duration::from_secs(300),
        )?;
        if !out.success() {
            return Err(lego_err(format!(
                "debopti-lego-run-once.sh failed (code {}): {}",
                out.code,
                out.stderr.trim()
            )));
        }
    }

    Ok(
        AppReport::new("lego", "run-once", true, "Lego run-once completed")
            .with_detail("primary_domain", primary_domain)
            .with_detail("mode", normalized_mode),
    )
}

/// Execute `/usr/local/bin/debopti-lego-hook.sh <primary_domain>` (directly as root or via `sudo`).
pub fn run_lego_ferron_hook(
    ctx: &AppContext,
    primary_domain: &str,
    runner: &dyn CommandRunner,
) -> Result<AppReport> {
    validate_primary_domain(primary_domain)?;
    let hook = ctx.path("/usr/local/bin/debopti-lego-hook.sh");
    let hook_str = hook.to_string_lossy().into_owned();

    if !ctx.no_sys {
        let out = if ctx.euid() == 0 {
            runner.run(&hook_str, &[primary_domain], Duration::from_secs(60))?
        } else {
            runner.run(
                "sudo",
                &[&hook_str, primary_domain],
                Duration::from_secs(60),
            )?
        };
        if !out.success() {
            return Err(lego_err(format!(
                "debopti-lego-hook.sh failed (code {}): {}",
                out.code,
                out.stderr.trim()
            )));
        }
    }

    Ok(
        AppReport::new("lego", "ferron-hook", true, "Lego Ferron hook executed")
            .with_detail("primary_domain", primary_domain),
    )
}

/// Remove ONLY `/etc/lego/envs/{primary}.env`, strictly preserving `/var/lib/lego/certificates/*` (`U8`).
pub fn remove_lego_domain(ctx: &AppContext, primary_domain: &str) -> Result<AppReport> {
    validate_primary_domain(primary_domain)?;
    let env_file = ctx.path(&format!("{LEGO_ENVS_DIR_ABS}/{primary_domain}.env"));
    if !env_file.exists() {
        return Err(lego_err(format!(
            "Lego domain configuration not found for '{primary_domain}'"
        )));
    }

    fs::remove_file(&env_file)?;
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "removed Lego domain env file while preserving certificates",
        None,
        &[("primary_domain", primary_domain)],
    );

    Ok(AppReport::new(
        "lego",
        "remove-domain",
        true,
        format!("Removed Lego configuration for {primary_domain}"),
    )
    .with_detail("primary_domain", primary_domain)
    .with_preserved(format!(
        "preserved certificate files under {LEGO_CERTS_DIR_ABS}/{primary_domain}.*"
    )))
}

/// Build the manual troubleshooting command for a configured domain without exposing API tokens (`U10`).
pub fn build_lego_troubleshoot_cmd(ctx: &AppContext, primary_domain: &str) -> Result<String> {
    validate_primary_domain(primary_domain)?;
    let env_file = ctx.path(&format!("{LEGO_ENVS_DIR_ABS}/{primary_domain}.env"));
    if !env_file.is_file() {
        return Err(lego_err(format!(
            "Lego env file not found for '{primary_domain}'"
        )));
    }

    let map = parse_lego_env_file(&env_file)?;
    let domains = map.get("DEBOPTI_DOMAINS").cloned().unwrap_or_default();
    let email = map.get("DEBOPTI_EMAIL").cloned().unwrap_or_default();
    let provider = map
        .get("DEBOPTI_PROVIDER")
        .cloned()
        .unwrap_or_else(default_provider);
    validate_provider(&provider)?;

    let display_env = format!("{LEGO_ENVS_DIR_ABS}/{primary_domain}.env");
    let mut domain_flags = String::new();
    for d in domains.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        validate_san_domain(d)?;
        domain_flags.push_str(&format!("  --domains=\"{}\" \\\n", escape_env_value(d)));
    }

    Ok(format!(
        concat!(
            "sudo DEBOPTI_INTERACTIVE_LEG=1 /usr/local/bin/debopti-lego-run-once.sh \"{env}\" issue\n",
            "\n",
            "sudo /usr/local/bin/lego run \\\n",
            "  --env-file=\"{env}\" \\\n",
            "  --email=\"{email}\" \\\n",
            "  --dns=\"{provider}\" \\\n",
            "{domain_flags}",
            "  --path=\"/var/lib/lego\" \\\n",
            "  --accept-tos \\\n",
            "  --dns.resolvers=1.1.1.1:53 \\\n",
            "  --dns.propagation.disable-rns"
        ),
        env = display_env,
        email = escape_env_value(&email),
        provider = provider,
        domain_flags = domain_flags
    ))
}
