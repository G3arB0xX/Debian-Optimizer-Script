use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::str::FromStr;
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

/// Fine-grained diagnostic severity level (ordered from most to least severe).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    #[default]
    Warn,
    Info,
    Debug,
    Trace,
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error => f.write_str("error"),
            Self::Warn => f.write_str("warn"),
            Self::Info => f.write_str("info"),
            Self::Debug => f.write_str("debug"),
            Self::Trace => f.write_str("trace"),
        }
    }
}

impl FromStr for LogLevel {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" | "err" => Ok(Self::Error),
            "warn" | "warning" => Ok(Self::Warn),
            "info" => Ok(Self::Info),
            "debug" => Ok(Self::Debug),
            "trace" => Ok(Self::Trace),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid log level '{other}': expected error|warn|info|debug|trace"
            ))),
        }
    }
}

/// Output wire format for diagnostic logs and CLI errors (always written to stderr).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => f.write_str("text"),
            Self::Json => f.write_str("json"),
        }
    }
}

impl FromStr for LogFormat {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid log format '{other}': expected text|json"
            ))),
        }
    }
}

/// Structured diagnostic record serialized as single-line text or NDJSON on `stderr`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagRecord {
    pub ts: String,
    pub level: LogLevel,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub msg: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

/// Process-wide diagnostic configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagConfig {
    pub level: LogLevel,
    pub format: LogFormat,
    pub no_color: bool,
}

impl Default for DiagConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Warn,
            format: LogFormat::Text,
            no_color: false,
        }
    }
}

fn global_config_cell() -> &'static RwLock<DiagConfig> {
    static CELL: OnceLock<RwLock<DiagConfig>> = OnceLock::new();
    CELL.get_or_init(|| RwLock::new(DiagConfig::default()))
}

/// Initialize or update the thread-safe global diagnostic configuration.
pub fn init_global(cfg: DiagConfig) {
    let mut guard = global_config_cell()
        .write()
        .unwrap_or_else(|e| e.into_inner());
    *guard = cfg;
}

/// Read the active process-wide diagnostic configuration.
pub fn current_config() -> DiagConfig {
    *global_config_cell()
        .read()
        .unwrap_or_else(|e| e.into_inner())
}

/// Parse an env-var level string supporting both plain levels (`"debug"`) and
/// `RUST_LOG`-style directives (`"debopty=debug,warn"`).
fn parse_env_log_level(raw: &str) -> Result<LogLevel> {
    let trimmed = raw.trim();
    if let Ok(level) = LogLevel::from_str(trimmed) {
        return Ok(level);
    }

    let mut matched: Option<LogLevel> = None;
    let mut fallback: Option<LogLevel> = None;
    for part in trimmed.split(',') {
        let token = part.trim();
        if token.is_empty() {
            continue;
        }
        if let Some((target, lvl)) = token.split_once('=') {
            if target.trim().starts_with("debopty") {
                matched = Some(LogLevel::from_str(lvl)?);
            }
        } else if let Ok(lvl) = LogLevel::from_str(token) {
            fallback = Some(lvl);
        }
    }

    matched.or(fallback).ok_or_else(|| {
        CoreError::ValidationFailed(format!(
            "invalid log level environment value '{trimmed}': expected error|warn|info|debug|trace"
        ))
    })
}

/// Resolve effective `LogLevel` with precedence:
/// `--log-level` > `--quiet` > `-v`/`--verbose` > `DEBOPTY_LOG`/`RUST_LOG` > `warn`.
pub fn resolve_level(
    cli_level: Option<&str>,
    verbose: u8,
    quiet: bool,
    env_val: Option<&str>,
) -> Result<LogLevel> {
    if let Some(raw) = cli_level.map(str::trim).filter(|s| !s.is_empty()) {
        return LogLevel::from_str(raw);
    }
    if quiet {
        return Ok(LogLevel::Error);
    }
    if verbose > 0 {
        return Ok(match verbose {
            1 => LogLevel::Info,
            2 => LogLevel::Debug,
            _ => LogLevel::Trace,
        });
    }
    if let Some(raw) = env_val.map(str::trim).filter(|s| !s.is_empty()) {
        return parse_env_log_level(raw);
    }
    Ok(LogLevel::Warn)
}

/// Resolve effective `LogFormat` with precedence: `--log-format` > `DEBOPTY_LOG_FORMAT` > `text`.
pub fn resolve_format(cli_format: Option<&str>, env_val: Option<&str>) -> Result<LogFormat> {
    if let Some(raw) = cli_format.map(str::trim).filter(|s| !s.is_empty()) {
        return LogFormat::from_str(raw);
    }
    if let Some(raw) = env_val.map(str::trim).filter(|s| !s.is_empty()) {
        return LogFormat::from_str(raw);
    }
    Ok(LogFormat::Text)
}

/// Convert days since 1970-01-01 to proleptic Gregorian `(year, month, day)`.
/// Based on Howard Hinnant's civil-from-days algorithm (zero external deps).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

/// Format Unix epoch seconds into RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
pub(crate) fn format_unix_secs_rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Current wall-clock timestamp formatted as RFC 3339 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn utc_rfc3339_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_unix_secs_rfc3339(secs)
}

fn pem_private_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        )
        .expect("valid pem regex")
    })
}

fn auth_header_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(Authorization\s*:\s*(?:Bearer|Basic)\s+)(\S+)")
            .expect("valid authorization header regex")
    })
}

fn secret_kv_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)\b((?:[A-Z0-9_-]*[_-])?(?:password|passwd|pass|secret|token|api[_-]?key|auth[_-]?key|private[_-]?key|license[_-]?key|access[_-]?key)[A-Z0-9_-]*|[A-Z0-9_-]*(?:password|passwd|secret|token|api[_-]?key|auth[_-]?key|private[_-]?key|license[_-]?key|access[_-]?key))(["']?)\s*([=:])\s*("(?:[^"\\]|\\.)*"|'[^']*'|\S+)"#,
        )
        .expect("valid secret kv regex")
    })
}

fn url_creds_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)([a-z][a-z0-9+.-]*://[^\s:/@]+:)([^\s/@]+)(@)")
            .expect("valid url creds regex")
    })
}

fn tskey_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\btskey-(?:auth|api|client)-[A-Za-z0-9_-]+").expect("valid tskey regex")
    })
}

fn github_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})")
            .expect("valid github token regex")
    })
}

fn gitlab_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\bglpat-[A-Za-z0-9_-]{20,}").expect("valid gitlab token regex"))
}

fn aws_access_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("valid aws access key regex")
    })
}

fn slack_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}").expect("valid slack token regex")
    })
}

fn slack_webhook_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"https://hooks\.slack\.com/services/T[A-Z0-9]+/B[A-Z0-9]+/[A-Za-z0-9]+")
            .expect("valid slack webhook regex")
    })
}

fn discord_webhook_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"https://(?:discord|discordapp)\.com/api/webhooks/[0-9]+/[A-Za-z0-9_-]+")
            .expect("valid discord webhook regex")
    })
}

fn age_secret_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\bAGE-SECRET-KEY-1[QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L]{58}\b")
            .expect("valid age secret key regex")
    })
}

fn telegram_bot_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b[0-9]{8,10}:AA[A-Za-z0-9_-]{33}\b").expect("valid telegram bot token regex")
    })
}

fn jwt_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{3,}(?:\.[A-Za-z0-9_-]*)?")
            .expect("valid jwt regex")
    })
}

fn set_credential_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b(SetCredential\s*=\s*[^:\s`]+:)([^\s`]+)")
            .expect("valid SetCredential regex")
    })
}

fn provider_sk_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"\b(?:sk-[A-Za-z0-9_-]{16,}|(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{12,})")
            .expect("valid provider sk key regex")
    })
}

fn cli_secret_flag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)(--(?:password|passwd|token|secret|api-key|authkey|auth-key|network-secret|cf-token|private-key|license-key)(?:=|\s+))("[^"]*"|'[^']*'|\S+)"#,
        )
        .expect("valid cli secret flag regex")
    })
}

fn chpasswd_quoted_before_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(["'])([a-z_][a-z0-9_-]*):([^"'\r\n]+)(["'])([^\n]*\bchpasswd\b)"#)
            .expect("valid chpasswd quoted before regex")
    })
}

fn chpasswd_unquoted_before_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)\b([a-z_][a-z0-9_-]*):([^"'&;|\r\n]+?)(\s*[|&;][^\n]*\bchpasswd\b)"#)
            .expect("valid chpasswd unquoted before regex")
    })
}

fn chpasswd_after_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)(chpasswd[^\n]*?\b[a-z_][a-z0-9_-]*):([^\s'&;|\x22]+)")
            .expect("valid chpasswd after regex")
    })
}

fn chpasswd_before_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\b([a-z_][a-z0-9_-]*):([^\s'&;|\x22]+)([^\n]*\bchpasswd\b)")
            .expect("valid chpasswd before regex")
    })
}

fn sensitive_field_key_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)(?:^|[_-])(password|passwd|pass|secret|token|api[_-]?key|auth[_-]?key|private[_-]?key|license[_-]?key|access[_-]?key|credential|cred|auth)(?:$|[_-])|(?:password|passwd|secret|token|api[_-]?key|auth[_-]?key|private[_-]?key|license[_-]?key|access[_-]?key)",
        )
        .expect("valid sensitive field key regex")
    })
}

fn is_sensitive_field_key(key: &str) -> bool {
    sensitive_field_key_re().is_match(key)
}

fn is_benign_metadata_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    const BENIGN_SUFFIXES: &[&str] = &[
        "_path",
        "-path",
        "_dir",
        "-dir",
        "_file",
        "-file",
        "_mode",
        "-mode",
        "_count",
        "-count",
        "_exists",
        "-exists",
        "_available",
        "-available",
        "_ok",
        "-ok",
        "_status",
        "-status",
        "_kind",
        "-kind",
        "_type",
        "-type",
        "_name",
        "-name",
        "_state",
        "-state",
        "_enabled",
        "-enabled",
        "_configured",
        "-configured",
        "_audit",
        "-audit",
        "_input",
        "-input",
        "_policy",
        "-policy",
        "_rules",
        "-rules",
        "_rule",
        "-rule",
        "_guide",
        "-guide",
        "_docs",
        "-docs",
        "_doc",
        "-doc",
        "_format",
        "-format",
        "_method",
        "-method",
        "_source",
        "-source",
        "_flags",
        "-flags",
        "_flag",
        "-flag",
    ];
    BENIGN_SUFFIXES.iter().any(|suf| lower.ends_with(suf))
}

fn is_sensitive_json_key(key: &str) -> bool {
    if is_benign_metadata_key(key) {
        return false;
    }
    is_sensitive_field_key(key)
}

fn is_placeholder_secret(raw: &str) -> bool {
    let trimmed = raw
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .trim_matches('`');
    trimmed.is_empty()
        || trimmed == "[REDACTED]"
        || trimmed == "[REDACTED_PRIVATE_KEY]"
        || (trimmed.starts_with("{{") && trimmed.ends_with("}}"))
        || (trimmed.starts_with("${") && trimmed.ends_with('}'))
        || (trimmed.starts_with('<') && trimmed.ends_with('>'))
}

/// Redact sensitive secrets (PEM private keys, Bearer/Basic auth headers, provider API tokens,
/// webhooks, Age keys, Telegram bot tokens, standalone JWTs, systemd `SetCredential=`,
/// prefixed/bare KV pairs, URL userinfo, Tailscale auth keys, CLI secret flags, and `chpasswd`
/// `user:pass` credentials) before emitting diagnostics or persisting logs/memory.
pub fn redact_sensitive(input: &str) -> String {
    let step = pem_private_key_re().replace_all(input, "[REDACTED_PRIVATE_KEY]");
    let step = auth_header_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        if is_placeholder_secret(&caps[2]) {
            caps[0].to_string()
        } else {
            format!("{}[REDACTED]", &caps[1])
        }
    });
    let step = url_creds_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        if is_placeholder_secret(&caps[2]) {
            caps[0].to_string()
        } else {
            format!("{}[REDACTED]{}", &caps[1], &caps[3])
        }
    });
    let step = tskey_re().replace_all(&step, "[REDACTED]");
    let step = github_token_re().replace_all(&step, "[REDACTED]");
    let step = gitlab_token_re().replace_all(&step, "[REDACTED]");
    let step = aws_access_key_re().replace_all(&step, "[REDACTED]");
    let step = slack_token_re().replace_all(&step, "[REDACTED]");
    let step = slack_webhook_re().replace_all(&step, "[REDACTED]");
    let step = discord_webhook_re().replace_all(&step, "[REDACTED]");
    let step = age_secret_key_re().replace_all(&step, "[REDACTED]");
    let step = telegram_bot_token_re().replace_all(&step, "[REDACTED]");
    let step = jwt_re().replace_all(&step, "[REDACTED]");
    let step = set_credential_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        if is_placeholder_secret(&caps[2]) {
            caps[0].to_string()
        } else {
            format!("{}[REDACTED]", &caps[1])
        }
    });
    let step = provider_sk_key_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        let m = &caps[0];
        if m.starts_with("sk-ssh-") || m.starts_with("sk-ecdsa-") {
            m.to_string()
        } else {
            "[REDACTED]".to_string()
        }
    });
    let step = secret_kv_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        if is_benign_metadata_key(&caps[1]) || is_placeholder_secret(&caps[4]) {
            caps[0].to_string()
        } else {
            format!("{}{}{}[REDACTED]", &caps[1], &caps[2], &caps[3])
        }
    });
    let step = cli_secret_flag_re().replace_all(&step, |caps: &regex::Captures<'_>| {
        if is_placeholder_secret(&caps[2]) {
            caps[0].to_string()
        } else {
            format!("{}[REDACTED]", &caps[1])
        }
    });
    let step = chpasswd_quoted_before_re().replace_all(&step, "${1}${2}:[REDACTED]${4}${5}");
    let step = chpasswd_unquoted_before_re().replace_all(&step, "${1}:[REDACTED]${3}");
    let step = chpasswd_after_re().replace_all(&step, "${1}:[REDACTED]");
    chpasswd_before_re()
        .replace_all(&step, "${1}:[REDACTED]${3}")
        .into_owned()
}

/// Recursively redact sensitive string values inside an arbitrary JSON tree while preserving
/// benign path/count/status/mode metadata keys.
pub fn redact_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(s) => {
            *s = redact_sensitive(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                if let serde_json::Value::String(s) = v {
                    if is_sensitive_json_key(k) {
                        if pem_private_key_re().is_match(s) {
                            *s = "[REDACTED_PRIVATE_KEY]".to_string();
                        } else if !is_placeholder_secret(s) {
                            *s = "[REDACTED]".to_string();
                        }
                    } else {
                        *s = redact_sensitive(s);
                    }
                } else {
                    redact_json_value(v);
                }
            }
        }
        _ => {}
    }
}

/// Return `true` iff `input` contains any secret material that `redact_sensitive` would mask.
pub fn contains_potential_secret(input: &str) -> bool {
    redact_sensitive(input) != input
}

fn sanitize_record(rec: &DiagRecord) -> DiagRecord {
    let mut fields = BTreeMap::new();
    for (k, v) in &rec.fields {
        let sanitized_val = if is_sensitive_field_key(k) {
            if pem_private_key_re().is_match(v) {
                "[REDACTED_PRIVATE_KEY]".to_string()
            } else {
                "[REDACTED]".to_string()
            }
        } else {
            redact_sensitive(v)
        };
        fields.insert(k.clone(), sanitized_val);
    }
    DiagRecord {
        ts: rec.ts.clone(),
        level: rec.level,
        target: rec.target.clone(),
        code: rec.code.clone(),
        msg: redact_sensitive(&rec.msg),
        hint: rec.hint.as_deref().map(redact_sensitive),
        fields,
    }
}

/// Format a `DiagRecord` into a single-line text or NDJSON representation (with redaction).
pub fn format_record(rec: &DiagRecord, fmt: LogFormat) -> String {
    let sanitized = sanitize_record(rec);
    match fmt {
        LogFormat::Json => serde_json::to_string(&sanitized).unwrap_or_else(|_| {
            format!(
                "{{\"ts\":\"{}\",\"level\":\"{}\",\"target\":\"{}\",\"msg\":\"{}\"}}",
                sanitized.ts, sanitized.level, sanitized.target, sanitized.msg
            )
        }),
        LogFormat::Text => {
            let level_tag = match sanitized.level {
                LogLevel::Error => "ERROR",
                LogLevel::Warn => "WARN",
                LogLevel::Info => "INFO",
                LogLevel::Debug => "DEBUG",
                LogLevel::Trace => "TRACE",
            };
            let mut out = if let Some(ref code) = sanitized.code {
                format!(
                    "[{}] [{}][{}] {}: {}",
                    sanitized.ts, level_tag, code, sanitized.target, sanitized.msg
                )
            } else {
                format!(
                    "[{}] [{}] {}: {}",
                    sanitized.ts, level_tag, sanitized.target, sanitized.msg
                )
            };
            if !sanitized.fields.is_empty() {
                let kv: Vec<String> = sanitized
                    .fields
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect();
                out.push_str(" (");
                out.push_str(&kv.join(" "));
                out.push(')');
            }
            if let Some(ref hint) = sanitized.hint
                && !hint.is_empty()
            {
                out.push_str("\n  hint: ");
                out.push_str(hint);
            }
            out
        }
    }
}

/// Deterministic diagnostic sink: writes `rec` to `writer` iff `rec.level <= cfg.level`.
pub fn emit_to<W: Write>(writer: &mut W, cfg: &DiagConfig, rec: &DiagRecord) {
    if rec.level > cfg.level {
        return;
    }
    let formatted = format_record(rec, cfg.format);
    let _ = writeln!(writer, "{formatted}");
}

/// Emit a structured diagnostic event exclusively to `stderr` using the global config.
pub fn log_event(
    level: LogLevel,
    target: &str,
    code: Option<&str>,
    msg: &str,
    hint: Option<&str>,
    fields: &[(&str, &str)],
) {
    let cfg = current_config();
    if level > cfg.level {
        return;
    }

    let mut field_map = BTreeMap::new();
    for &(k, v) in fields {
        field_map.insert(k.to_string(), v.to_string());
    }

    let rec = DiagRecord {
        ts: utc_rfc3339_now(),
        level,
        target: target.to_string(),
        code: code.map(str::to_string),
        msg: msg.to_string(),
        hint: hint.map(str::to_string),
        fields: field_map,
    };

    let stderr = io::stderr();
    let mut handle = stderr.lock();
    emit_to(&mut handle, &cfg, &rec);
}

/// Format a top-level CLI error for `stderr` output in either `text` or NDJSON `json` format.
pub fn format_cli_error(code: &str, msg: &str, hint: &str, fmt: LogFormat) -> String {
    let clean_msg = redact_sensitive(msg);
    let clean_hint = redact_sensitive(hint);
    match fmt {
        LogFormat::Text => format!("[ERROR][{code}] debopty: {clean_msg}\n  hint: {clean_hint}"),
        LogFormat::Json => {
            let rec = DiagRecord {
                ts: utc_rfc3339_now(),
                level: LogLevel::Error,
                target: "debopty::cli".to_string(),
                code: Some(code.to_string()),
                msg: clean_msg,
                hint: Some(clean_hint),
                fields: BTreeMap::new(),
            };
            serde_json::to_string(&rec).unwrap_or_else(|_| {
                format!(
                    "{{\"level\":\"error\",\"code\":\"{code}\",\"target\":\"debopty::cli\",\"msg\":\"error\"}}"
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_level_and_format_precedence() {
        assert_eq!(resolve_level(None, 0, false, None).unwrap(), LogLevel::Warn);
        assert_eq!(
            resolve_level(None, 0, false, Some("info")).unwrap(),
            LogLevel::Info
        );
        assert_eq!(
            resolve_level(None, 0, false, Some("debopty=debug,warn")).unwrap(),
            LogLevel::Debug
        );
        assert_eq!(
            resolve_level(None, 1, false, Some("error")).unwrap(),
            LogLevel::Info
        );
        assert_eq!(
            resolve_level(None, 2, false, None).unwrap(),
            LogLevel::Debug
        );
        assert_eq!(
            resolve_level(None, 4, false, None).unwrap(),
            LogLevel::Trace
        );
        assert_eq!(resolve_level(None, 3, true, None).unwrap(), LogLevel::Error);
        assert_eq!(
            resolve_level(Some("trace"), 0, true, Some("error")).unwrap(),
            LogLevel::Trace
        );
        assert!(resolve_level(Some("invalid"), 0, false, None).is_err());

        assert_eq!(resolve_format(None, None).unwrap(), LogFormat::Text);
        assert_eq!(resolve_format(None, Some("json")).unwrap(), LogFormat::Json);
        assert_eq!(
            resolve_format(Some("text"), Some("json")).unwrap(),
            LogFormat::Text
        );
        assert!(resolve_format(Some("xml"), None).is_err());
    }

    #[test]
    fn utc_rfc3339_epoch_and_leap_day_accuracy() {
        assert_eq!(format_unix_secs_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(
            format_unix_secs_rfc3339(1_709_164_800),
            "2024-02-29T00:00:00Z"
        );
        let now = utc_rfc3339_now();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z'));
        assert_eq!(&now[10..11], "T");
    }

    fn synth_pem(label: &str, body: &str) -> String {
        let dash = "-".repeat(5);
        let beg = ["BEG", "IN"].concat();
        let end = ["EN", "D"].concat();
        format!("{dash}{beg} {label}{dash}\n{body}\n{dash}{end} {label}{dash}")
    }

    #[test]
    fn redact_sensitive_masks_passwords_tokens_chpasswd_and_pem_keys() {
        let jwt_hdr = ["eyJ", "hbGciOiJIUzI1NiJ9"].concat();
        let jwt_full = format!("{jwt_hdr}.abc");
        let openssh_pem = synth_pem(&["OPENSSH ", "PRIVATE ", "KEY"].concat(), "abcdef123456");
        let sample = format!(
            "password=SuperSecret123 pass:hunter2 Authorization: Bearer {jwt_full}\n\
             echo alice:MyPass99 | chpasswd\n\
             chpasswd failed for bob:TopSecret!\n\
             --password MyCliSecret\n\
             {openssh_pem}"
        );
        let redacted = redact_sensitive(&sample);
        assert!(!redacted.contains("SuperSecret123"));
        assert!(!redacted.contains("hunter2"));
        assert!(!redacted.contains(&jwt_hdr));
        assert!(!redacted.contains("TopSecret!"));
        assert!(!redacted.contains("MyCliSecret"));
        assert!(!redacted.contains("abcdef123456"));
        assert!(redacted.contains("[REDACTED_PRIVATE_KEY]"));
        assert!(redacted.contains("Authorization: Bearer [REDACTED]"));
        assert!(redacted.contains("password=[REDACTED]"));

        // JSON-quoted keys (double and single quoted) with spaces or compact colons
        let sk_tok = ["sk", "-xyz"].concat();
        let json_sample = format!(
            r#"{{"cf_token": "secret123", "api_key":"{sk_tok}", 'password': 'my secret', "ok": "keep"}}"#
        );
        let json_redacted = redact_sensitive(&json_sample);
        assert!(!json_redacted.contains("secret123"));
        assert!(!json_redacted.contains(&sk_tok));
        assert!(!json_redacted.contains("my secret"));
        assert!(json_redacted.contains(r#""ok": "keep""#));
    }

    #[test]
    fn emit_to_filters_by_level_and_formats_text_and_json() {
        let rec = DiagRecord {
            ts: "2026-11-22T03:00:00Z".to_string(),
            level: LogLevel::Debug,
            target: "debopty::reconcile::ssh".to_string(),
            code: Some("E5002".to_string()),
            msg: "checking password=secret123".to_string(),
            hint: Some("检查配置".to_string()),
            fields: BTreeMap::from([("target".to_string(), "/etc/ssh/sshd_config".to_string())]),
        };

        // Filtered out when cfg.level == Warn
        let mut buf_warn = Vec::new();
        emit_to(
            &mut buf_warn,
            &DiagConfig {
                level: LogLevel::Warn,
                format: LogFormat::Json,
                no_color: true,
            },
            &rec,
        );
        assert!(buf_warn.is_empty());

        // Emitted as single-line NDJSON when cfg.level == Debug
        let mut buf_json = Vec::new();
        emit_to(
            &mut buf_json,
            &DiagConfig {
                level: LogLevel::Debug,
                format: LogFormat::Json,
                no_color: true,
            },
            &rec,
        );
        let line = String::from_utf8(buf_json).unwrap();
        assert_eq!(line.lines().count(), 1);
        let parsed: DiagRecord = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed.code.as_deref(), Some("E5002"));
        assert_eq!(parsed.msg, "checking password=[REDACTED]");

        // CLI error format text & json
        let cli_txt = format_cli_error("E5001", "boom pass=123", "请重试", LogFormat::Text);
        assert!(cli_txt.starts_with("[ERROR][E5001] debopty: boom pass=[REDACTED]"));
        assert!(cli_txt.contains("  hint: 请重试"));

        let cli_json = format_cli_error("E5001", "boom", "请重试", LogFormat::Json);
        let parsed_cli: DiagRecord = serde_json::from_str(&cli_json).unwrap();
        assert_eq!(parsed_cli.level, LogLevel::Error);
        assert_eq!(parsed_cli.target, "debopty::cli");
        assert_eq!(parsed_cli.code.as_deref(), Some("E5001"));
    }

    #[test]
    fn redact_sensitive_masks_u29_tokens_and_preserves_fido2_ssh_keys() {
        let gh = format!("{}_{}", ["gh", "p"].concat(), "a".repeat(24));
        let gh_pat = format!("{}_{}", ["github", "_", "pat"].concat(), "b".repeat(24));
        let gl = format!("{}-{}", ["gl", "pat"].concat(), "c".repeat(22));
        let aws = format!("{}{}", ["AK", "IA"].concat(), "1234567890ABCDEF");
        let slack_tok = format!("{}-{}", ["xox", "b"].concat(), "1234567890-abcdef");
        let slack_hook = format!(
            "https://{}.{}/services/T01234567/B89ABCDEF/xyz123456789",
            ["hooks", ".", "slack"].concat(),
            "com"
        );
        let discord_hook = format!(
            "https://{}.com/api/{}/1234567890/AbCdEfGhIjKlMnOp",
            ["dis", "cord"].concat(),
            ["web", "hooks"].concat()
        );
        let age = format!(
            "{}-1{}",
            ["AGE-SECRET", "-", "KEY"].concat(),
            "Q".repeat(58)
        );
        let tg = format!("123456789:{}{}", "AA", "z".repeat(33));
        let jwt = format!(
            "{}.{}.{}",
            ["eyJ", "hbGciOiJIUzI1NiJ9"].concat(),
            "payloadPart123",
            "sigPart456"
        );
        let set_cred = format!("SetCredential=cfapi:{}", ["plain", "Secret99"].concat());
        let sk_openai = format!("{}-{}", "sk", "proj1234567890abcdef");
        let sk_live = format!("{}_{}_{}", "sk", "live", "1234567890abcdef");
        let rk_live = format!("{}_{}_{}", "rk", "live", "1234567890abcdef1234");
        let pk_live = format!("{}_{}_{}", "pk", "live", "1234567890abcdef1234");
        let fido_ed = "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5";
        let fido_ec = "sk-ecdsa-sha2-nistp256@openssh.com AAAAE2VjZHNh";

        for tok in [
            &gh,
            &gh_pat,
            &gl,
            &aws,
            &slack_tok,
            &slack_hook,
            &discord_hook,
            &age,
            &tg,
            &jwt,
            &set_cred,
            &sk_openai,
            &sk_live,
            &rk_live,
            &pk_live,
        ] {
            assert!(
                contains_potential_secret(tok),
                "expected secret detection for pattern"
            );
            let out = redact_sensitive(tok);
            assert!(out.contains("[REDACTED]"));
            assert_ne!(&out, tok);
            assert!(
                !contains_potential_secret(&out),
                "redacted output must be idempotent: {out}"
            );
        }

        // OpenSSH FIDO2 key types must NEVER be mangled or flagged as secrets
        assert_eq!(redact_sensitive(fido_ed), fido_ed);
        assert_eq!(redact_sensitive(fido_ec), fido_ec);
        assert!(!contains_potential_secret(fido_ed));
        assert!(!contains_potential_secret(fido_ec));

        // Placeholder SetCredential docs must not false-positive
        let doc_example = "`SetCredential=<name>:<plaintext>`";
        assert_eq!(redact_sensitive(doc_example), doc_example);
        assert!(!contains_potential_secret(doc_example));
    }

    #[test]
    fn redact_json_value_redacts_secrets_and_preserves_benign_metadata_keys() {
        let gh = format!("{}_{}", ["gh", "p"].concat(), "Z".repeat(24));
        let pem = synth_pem(&["RSA ", "PRIVATE ", "KEY"].concat(), "MIIEowIBAAKCAQEA");
        let mut val = serde_json::json!({
            "cred_dir": "/var/lib/debopty/agent/.agents/credentials",
            "cred_dir_exists": true,
            "cred_dir_mode": 448,
            "token_path": "/etc/debopti/credentials/cf",
            "password_status": "disabled",
            "auth_count": 2,
            "api_token": "plain-secret-value",
            "private_key": pem,
            "nested": {
                "summary": format!("leaked token {gh} here"),
                "secret": "nested-secret"
            },
            "list": [format!("raw {gh}"), "clean-item"]
        });

        redact_json_value(&mut val);

        assert_eq!(
            val["cred_dir"],
            "/var/lib/debopty/agent/.agents/credentials"
        );
        assert_eq!(val["cred_dir_exists"], true);
        assert_eq!(val["cred_dir_mode"], 448);
        assert_eq!(val["token_path"], "/etc/debopti/credentials/cf");
        assert_eq!(val["password_status"], "disabled");
        assert_eq!(val["auth_count"], 2);
        assert_eq!(val["api_token"], "[REDACTED]");
        assert_eq!(val["private_key"], "[REDACTED_PRIVATE_KEY]");
        assert_eq!(val["nested"]["secret"], "[REDACTED]");
        assert_eq!(val["nested"]["summary"], "leaked token [REDACTED] here");
        assert_eq!(val["list"][0], "raw [REDACTED]");
        assert_eq!(val["list"][1], "clean-item");
    }
}
