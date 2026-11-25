use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::sys::{self, CommandRunner, RealCommandRunner};
use crate::template::TemplateEngine;

pub const DEFAULT_CRED_DIR_REL: &str = ".agents/credentials";
pub const SYSTEM_CRED_DIR: &str = "/etc/debopti/credentials";
pub const CRED_DIR_MODE: u32 = 0o700;
pub const CRED_FILE_MODE: u32 = 0o600;

const DIAG_TARGET: &str = "debopty::credentials";
const MAX_AUDIT_SCAN_BYTES: u64 = 2 * 1024 * 1024;
const DROPIN_TEMPLATE_REL: &str = "templates/agent/systemd_credential_dropin.conf.tmpl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    RawFile,
    EnvFile,
    SystemdEncrypted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMeta {
    pub name: String,
    pub path: String,
    pub kind: CredentialKind,
    pub mode: u32,
    pub size_bytes: u64,
    pub encrypted: bool,
    pub perms_ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialFinding {
    pub severity: String, // "error" | "warn"
    pub path: String,
    pub code: String,
    pub message: String, // NEVER echoes raw secret material
    pub remediation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialAuditReport {
    pub cred_dir: String,
    pub cred_dir_exists: bool,
    pub cred_dir_mode: Option<u32>,
    pub systemd_creds_available: bool,
    pub credentials: Vec<CredentialMeta>,
    pub findings: Vec<CredentialFinding>,
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemdCredSnippet {
    pub unit: String,
    pub directive_kind: String,
    pub dropin_path: String,
    pub content: String,
}

fn systemd_section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\[([^\]]+)\]\s*$").expect("valid systemd section regex"))
}

fn systemd_set_cred_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\s*SetCredential\s*=\s*([^:\s]+):(.*)$").expect("valid SetCredential regex")
    })
}

fn systemd_env_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*Environment\s*=\s*(.+)$").expect("valid Environment regex"))
}

fn systemd_env_secret_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?:^|[\s"'])([A-Z0-9_]*(?:PASSWORD|TOKEN|SECRET|API_KEY))=([^\s"']*)"#)
            .expect("valid systemd env secret regex")
    })
}

fn is_placeholder_val(raw: &str) -> bool {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    trimmed.is_empty()
        || trimmed.contains("{{")
        || trimmed.contains("${")
        || trimmed.contains("<")
        || trimmed.contains("[REDACTED]")
}

/// Validate a logical credential identifier (alphanumeric start, safe `[A-Za-z0-9._-]` body,
/// max 64 chars, no path traversal or control characters).
pub fn validate_credential_name(name: &str) -> Result<String> {
    if name.is_empty()
        || name.len() > 64
        || name.contains("..")
        || name.contains(['/', '\\', '\0', '\r', '\n'])
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid credential name '{name}'"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap_or('\0');
    if !first.is_ascii_alphanumeric() {
        return Err(CoreError::ValidationFailed(format!(
            "credential name must start with an ASCII letter or digit: '{name}'"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
        return Err(CoreError::ValidationFailed(format!(
            "credential name contains unsupported characters: '{name}'"
        )));
    }
    Ok(name.to_string())
}

fn validate_unit_name(unit: &str) -> Result<String> {
    let trimmed = unit.trim();
    if trimmed.is_empty()
        || trimmed.len() > 128
        || trimmed.starts_with('-')
        || trimmed.contains("..")
        || trimmed.contains(['/', '\\', '\0', '\r', '\n', ' '])
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '-'))
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid systemd unit name '{unit}'"
        )));
    }
    let base = trimmed.strip_suffix(".service").unwrap_or(trimmed);
    if base.is_empty() {
        return Err(CoreError::ValidationFailed(format!(
            "invalid systemd unit name '{unit}'"
        )));
    }
    Ok(format!("{base}.service"))
}

fn cred_filename(valid_name: &str, kind: CredentialKind) -> String {
    match kind {
        CredentialKind::RawFile => valid_name.to_string(),
        CredentialKind::EnvFile => {
            if valid_name.ends_with(".env") {
                valid_name.to_string()
            } else {
                format!("{valid_name}.env")
            }
        }
        CredentialKind::SystemdEncrypted => {
            if valid_name.ends_with(".cred") {
                valid_name.to_string()
            } else {
                format!("{valid_name}.cred")
            }
        }
    }
}

/// Ensure credential storage directory exists with strict `0700` permissions.
pub fn ensure_credential_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(CRED_DIR_MODE))?;
    Ok(())
}

/// Store a credential atomically under `dir` with `0600` permissions.
///
/// For `CredentialKind::SystemdEncrypted`, invokes `systemd-creds encrypt --name=<name> <tmp> <dest.cred>`
/// via `runner` and fails closed if `systemd-creds` is unavailable or errors (never silently
/// downgrading to plaintext).
pub fn store_credential(
    dir: &Path,
    name: &str,
    secret_bytes: &[u8],
    kind: CredentialKind,
    runner: Option<&dyn CommandRunner>,
) -> Result<CredentialMeta> {
    let valid_name = validate_credential_name(name)?;
    if secret_bytes.is_empty() {
        return Err(CoreError::ValidationFailed(
            "credential payload must not be empty".to_string(),
        ));
    }

    ensure_credential_dir(dir)?;
    let dest = dir.join(cred_filename(&valid_name, kind));

    match kind {
        CredentialKind::RawFile | CredentialKind::EnvFile => {
            sys::write_file_atomic(&dest, secret_bytes, Some(CRED_FILE_MODE))?;
        }
        CredentialKind::SystemdEncrypted => {
            let real_runner = RealCommandRunner;
            let eff_runner: &dyn CommandRunner = runner.unwrap_or(&real_runner);

            let mut tmp = NamedTempFile::new_in(dir)?;
            fs::set_permissions(tmp.path(), fs::Permissions::from_mode(CRED_FILE_MODE))?;
            tmp.write_all(secret_bytes)?;
            tmp.flush()?;

            let name_flag = format!("--name={valid_name}");
            let tmp_str = tmp.path().to_string_lossy().into_owned();
            let dest_str = dest.to_string_lossy().into_owned();
            let args = [
                "encrypt",
                name_flag.as_str(),
                tmp_str.as_str(),
                dest_str.as_str(),
            ];

            let out = eff_runner
                .run("systemd-creds", &args, Duration::from_secs(30))
                .map_err(|e| {
                    CoreError::Security(format!(
                        "systemd-creds 不可用，已拒绝静默降级为明文存储（请安装 systemd >= 250 或显式使用 raw/env 类型）: {e}"
                    ))
                })?;

            // Drop temp plaintext immediately before inspecting/finishing destination.
            drop(tmp);

            if !out.success() {
                let _ = fs::remove_file(&dest);
                let detail = diag::redact_sensitive(out.stderr.trim());
                return Err(CoreError::Security(format!(
                    "systemd-creds encrypt 执行失败 (code={})，已拒绝明文降级: {detail}",
                    out.code
                )));
            }

            if dest.is_file() {
                fs::set_permissions(&dest, fs::Permissions::from_mode(CRED_FILE_MODE))?;
            } else if !out.stdout.is_empty() {
                // Supports deterministic unit-test runners that return encrypted payload via stdout.
                sys::write_file_atomic(&dest, out.stdout.as_bytes(), Some(CRED_FILE_MODE))?;
            } else {
                return Err(CoreError::Security(
                    "systemd-creds encrypt 未生成密文输出文件，已按安全失败策略中止".to_string(),
                ));
            }
        }
    }

    let meta = fs::metadata(&dest)?;
    let mode = meta.permissions().mode() & 0o7777;
    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "stored credential metadata",
        None,
        &[
            ("name", &valid_name),
            ("path", &dest.to_string_lossy()),
            ("mode", &format!("{mode:04o}")),
        ],
    );

    Ok(CredentialMeta {
        name: valid_name,
        path: dest.to_string_lossy().into_owned(),
        kind,
        mode,
        size_bytes: meta.len(),
        encrypted: kind == CredentialKind::SystemdEncrypted,
        perms_ok: mode == CRED_FILE_MODE,
    })
}

/// Remove a stored credential (matching raw, `.env`, or `.cred` forms) under `dir`.
pub fn remove_credential(dir: &Path, name: &str) -> Result<bool> {
    let valid_name = validate_credential_name(name)?;
    let candidates = [
        dir.join(cred_filename(&valid_name, CredentialKind::RawFile)),
        dir.join(cred_filename(&valid_name, CredentialKind::EnvFile)),
        dir.join(cred_filename(&valid_name, CredentialKind::SystemdEncrypted)),
    ];

    let mut removed = false;
    for path in candidates {
        if fs::symlink_metadata(&path).is_ok() {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    Ok(removed)
}

/// List credential metadata inside `dir` without ever reading credential file contents.
pub fn list_credentials(dir: &Path) -> Result<Vec<CredentialMeta>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut items = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let fname = entry.file_name();
        let Some(fname_str) = fname.to_str() else {
            continue;
        };
        if fname_str.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            continue;
        }
        let (kind, logical_name) = if let Some(stem) = fname_str.strip_suffix(".cred") {
            (CredentialKind::SystemdEncrypted, stem.to_string())
        } else if let Some(stem) = fname_str.strip_suffix(".env") {
            (CredentialKind::EnvFile, stem.to_string())
        } else {
            (CredentialKind::RawFile, fname_str.to_string())
        };
        let mode = meta.permissions().mode() & 0o7777;
        items.push(CredentialMeta {
            name: logical_name,
            path: path.to_string_lossy().into_owned(),
            kind,
            mode,
            size_bytes: meta.len(),
            encrypted: kind == CredentialKind::SystemdEncrypted,
            perms_ok: mode == CRED_FILE_MODE,
        });
    }

    items.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    Ok(items)
}

/// Render a hardened systemd credential drop-in snippet from `templates/agent/systemd_credential_dropin.conf.tmpl`.
pub fn render_systemd_dropin(
    tpl: &TemplateEngine,
    unit: &str,
    cred_name: &str,
    cred_path: &Path,
    kind: CredentialKind,
) -> Result<SystemdCredSnippet> {
    let unit_name = validate_unit_name(unit)?;
    let valid_cred = validate_credential_name(cred_name)?;
    let path_str = cred_path.to_string_lossy();
    if path_str.is_empty() || path_str.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(
            "credential path must be non-empty and free of control characters".to_string(),
        ));
    }

    let (directive_kind, directive_line) = match kind {
        CredentialKind::RawFile => (
            "LoadCredential",
            format!("LoadCredential={valid_cred}:{path_str}"),
        ),
        CredentialKind::SystemdEncrypted => (
            "LoadCredentialEncrypted",
            format!("LoadCredentialEncrypted={valid_cred}:{path_str}"),
        ),
        CredentialKind::EnvFile => ("EnvironmentFile", format!("EnvironmentFile={path_str}")),
    };

    let mut vars = BTreeMap::new();
    vars.insert("UNIT_NAME".to_string(), unit_name.clone());
    vars.insert("CREDENTIAL_DIRECTIVE".to_string(), directive_line);

    let content = tpl.render_to_string(DROPIN_TEMPLATE_REL, &vars)?;
    let dropin_path = format!("/etc/systemd/system/{unit_name}.d/90-debopty-credentials.conf");

    Ok(SystemdCredSnippet {
        unit: unit_name,
        directive_kind: directive_kind.to_string(),
        dropin_path,
        content,
    })
}

fn check_systemd_creds_available(runner: Option<&dyn CommandRunner>) -> bool {
    if let Some(r) = runner {
        return r
            .run("systemd-creds", &["--version"], Duration::from_secs(5))
            .map(|o| o.success())
            .unwrap_or(false);
    }

    for dir in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let candidate = Path::new(dir).join("systemd-creds");
        if candidate.is_file()
            && fs::metadata(&candidate)
                .map(|m| m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        {
            return true;
        }
    }
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let candidate = dir.join("systemd-creds");
            if candidate.is_file()
                && fs::metadata(&candidate)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            {
                return true;
            }
        }
    }
    false
}

fn json_non_terms_are_clean(val: &serde_json::Value) -> bool {
    match val {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
        serde_json::Value::String(s) => !diag::contains_potential_secret(s),
        serde_json::Value::Array(items) => items.iter().all(json_non_terms_are_clean),
        serde_json::Value::Object(map) => map.iter().all(|(k, v)| {
            if let serde_json::Value::String(s) = v
                && diag::is_sensitive_json_key_public(k)
                && !s.trim().is_empty()
            {
                return false;
            }
            !diag::contains_potential_secret(k) && json_non_terms_are_clean(v)
        }),
    }
}

fn is_safe_bm25_doc_entry(doc: &serde_json::Value) -> bool {
    const ALLOWED_DOC_KEYS: &[&str] = &[
        "rel_path",
        "path",
        "mtime_secs",
        "size_bytes",
        "sha256",
        "meta",
        "doc_len",
        "terms",
        "line_count",
        "id",
        "title",
        "scope",
        "tags",
    ];
    let Some(obj) = doc.as_object() else {
        return false;
    };
    let Some(terms) = obj.get("terms").and_then(|v| v.as_object()) else {
        return false;
    };
    for (k, v) in obj {
        if !ALLOWED_DOC_KEYS.contains(&k.as_str()) {
            return false;
        }
        if k == "terms" {
            continue;
        }
        if !json_non_terms_are_clean(v) {
            return false;
        }
    }
    terms
        .iter()
        .all(|(term, freq)| freq.as_u64().is_some() && !diag::contains_high_confidence_secret(term))
}

/// Verify whether `path` is `.agents/cache/memory_index.json` and `content` strictly conforms
/// to the BM25 term-frequency cache schema without injected plaintext secrets.
pub(crate) fn is_safe_bm25_memory_index(path: &Path, content: &str) -> bool {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if !(normalized == ".agents/cache/memory_index.json"
        || normalized.ends_with("/.agents/cache/memory_index.json"))
    {
        return false;
    }
    if diag::contains_high_confidence_secret(content) {
        return false;
    }
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str::<serde_json::Value>(content)
    else {
        return false;
    };
    const ALLOWED_ROOT_KEYS: &[&str] = &[
        "schema_version",
        "generated_at",
        "doc_count",
        "avg_doc_len",
        "docs",
    ];
    if !root.keys().all(|k| ALLOWED_ROOT_KEYS.contains(&k.as_str())) {
        return false;
    }
    if root
        .get("schema_version")
        .is_none_or(|v| v.as_u64().is_none())
        || root.get("doc_count").is_none_or(|v| v.as_u64().is_none())
        || root.get("avg_doc_len").is_none_or(|v| !v.is_number())
    {
        return false;
    }
    if let Some(gen_at) = root.get("generated_at")
        && !json_non_terms_are_clean(gen_at)
    {
        return false;
    }
    match root.get("docs") {
        Some(serde_json::Value::Object(docs_map)) => docs_map
            .iter()
            .all(|(k, v)| !diag::contains_potential_secret(k) && is_safe_bm25_doc_entry(v)),
        Some(serde_json::Value::Array(docs_arr)) => docs_arr.iter().all(is_safe_bm25_doc_entry),
        _ => false,
    }
}

fn scan_file_for_credential_leaks(file_path: &Path, findings: &mut Vec<CredentialFinding>) {
    let Ok(meta) = fs::symlink_metadata(file_path) else {
        return;
    };
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > MAX_AUDIT_SCAN_BYTES {
        return;
    }
    let Ok(raw) = fs::read(file_path) else {
        return;
    };
    if raw.contains(&0) {
        return;
    }
    let text = String::from_utf8_lossy(&raw);
    if is_safe_bm25_memory_index(file_path, &text) {
        return;
    }
    let path_str = file_path.to_string_lossy().into_owned();
    let fname = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    let is_systemd_unit = fname.ends_with(".service") || fname.ends_with(".conf");

    let mut in_service = false;
    let mut reported_generic_leak = false;

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        if is_systemd_unit {
            if let Some(caps) = systemd_section_re().captures(line) {
                in_service = caps[1].trim() == "Service";
                continue;
            }
            if in_service {
                if let Some(caps) = systemd_set_cred_re().captures(line)
                    && !is_placeholder_val(&caps[2])
                {
                    findings.push(CredentialFinding {
                        severity: "error".to_string(),
                        path: path_str.clone(),
                        code: "SYSTEMD_INLINE_SET_CREDENTIAL".to_string(),
                        message: format!(
                            "第 {lineno} 行存在明文 SetCredential 指令（已抑制原文回显）"
                        ),
                        remediation:
                            "改用 LoadCredentialEncrypted= 或 LoadCredential= 引用 0600 凭据文件"
                                .to_string(),
                    });
                    continue;
                }
                if let Some(caps) = systemd_env_line_re().captures(line) {
                    for sm in systemd_env_secret_re().captures_iter(&caps[1]) {
                        if !is_placeholder_val(&sm[2]) {
                            findings.push(CredentialFinding {
                                severity: "error".to_string(),
                                path: path_str.clone(),
                                code: "SYSTEMD_INLINE_ENV_SECRET".to_string(),
                                message: format!(
                                    "第 {lineno} 行 Environment={} 包含明文敏感值（已抑制原文回显）",
                                    &sm[1]
                                ),
                                remediation:
                                    "将密钥迁移至 0600 的 EnvironmentFile= 或 LoadCredential="
                                        .to_string(),
                            });
                            break;
                        }
                    }
                }
            }
        }

        if !reported_generic_leak && diag::contains_potential_secret(line) {
            reported_generic_leak = true;
            findings.push(CredentialFinding {
                severity: "error".to_string(),
                path: path_str.clone(),
                code: "SECRET_LEAK".to_string(),
                message: format!("第 {lineno} 行检测到疑似明文凭据或敏感密钥（已抑制原文回显）"),
                remediation: "移除明文密钥并通过 `debopty agent cred set` 隔离保存".to_string(),
            });
        }
    }
}

/// Audit credential directory permissions (`0700`), stored credential file modes (`0600`),
/// `systemd-creds` availability, and scan `extra_scan_paths` for inline secrets without ever
/// echoing raw secret material.
pub fn audit_credentials(
    cred_dir: &Path,
    extra_scan_paths: &[PathBuf],
    runner: Option<&dyn CommandRunner>,
) -> CredentialAuditReport {
    let systemd_creds_available = check_systemd_creds_available(runner);
    let cred_dir_str = cred_dir.to_string_lossy().into_owned();
    let cred_dir_exists = cred_dir.is_dir();
    let mut findings = Vec::new();
    let mut credentials = Vec::new();

    let cred_dir_mode = if cred_dir_exists {
        let mode = fs::metadata(cred_dir)
            .ok()
            .map(|m| m.permissions().mode() & 0o7777);
        if mode != Some(CRED_DIR_MODE) {
            findings.push(CredentialFinding {
                severity: "error".to_string(),
                path: cred_dir_str.clone(),
                code: "CRED_DIR_PERMS".to_string(),
                message: format!(
                    "凭据目录权限应为 {:04o}，当前为 {}",
                    CRED_DIR_MODE,
                    mode.map(|m| format!("{m:04o}"))
                        .unwrap_or_else(|| "unknown".to_string())
                ),
                remediation: format!("chmod 0700 {}", cred_dir.display()),
            });
        }
        match list_credentials(cred_dir) {
            Ok(list) => {
                for cred in &list {
                    if !cred.perms_ok {
                        findings.push(CredentialFinding {
                            severity: "error".to_string(),
                            path: cred.path.clone(),
                            code: "CRED_FILE_PERMS".to_string(),
                            message: format!(
                                "凭据文件 {} 权限应为 {:04o}，当前为 {:04o}",
                                cred.name, CRED_FILE_MODE, cred.mode
                            ),
                            remediation: format!("chmod 0600 {}", cred.path),
                        });
                    }
                }
                credentials = list;
            }
            Err(e) => {
                findings.push(CredentialFinding {
                    severity: "error".to_string(),
                    path: cred_dir_str.clone(),
                    code: "CRED_DIR_UNREADABLE".to_string(),
                    message: format!("无法枚举凭据目录元数据: {e}"),
                    remediation: format!("检查目录所有权与权限: {}", cred_dir.display()),
                });
            }
        }
        mode
    } else {
        findings.push(CredentialFinding {
            severity: "error".to_string(),
            path: cred_dir_str.clone(),
            code: "CRED_DIR_MISSING".to_string(),
            message: "凭据隔离目录不存在".to_string(),
            remediation: "运行 `debopty agent init` 初始化受控凭据目录".to_string(),
        });
        None
    };

    let cred_dir_canon = fs::canonicalize(cred_dir).ok();
    for scan_root in extra_scan_paths {
        if !scan_root.exists() {
            continue;
        }
        if scan_root.is_file() {
            let inside_cred = cred_dir_canon
                .as_ref()
                .zip(fs::canonicalize(scan_root).ok())
                .is_some_and(|(cd, p)| p.starts_with(cd))
                || scan_root.starts_with(cred_dir);
            if !inside_cred {
                scan_file_for_credential_leaks(scan_root, &mut findings);
            }
            continue;
        }
        for entry in
            WalkDir::new(scan_root)
                .follow_links(false)
                .into_iter()
                .filter_entry(|e| {
                    if e.path() == cred_dir {
                        return false;
                    }
                    if let (Some(cd), Ok(p)) = (&cred_dir_canon, fs::canonicalize(e.path()))
                        && p.starts_with(cd)
                    {
                        return false;
                    }
                    if e.path().strip_prefix(scan_root).ok().is_some_and(|r| {
                        r.starts_with(".agents/checkpoints") || r.starts_with(".git")
                    }) || e.path().ends_with(".agents/checkpoints")
                    {
                        return false;
                    }
                    true
                })
                .filter_map(std::result::Result::ok)
        {
            if entry.file_type().is_file() {
                scan_file_for_credential_leaks(entry.path(), &mut findings);
            }
        }
    }

    let ok = cred_dir_exists && !findings.iter().any(|f| f.severity == "error");
    CredentialAuditReport {
        cred_dir: cred_dir_str,
        cred_dir_exists,
        cred_dir_mode,
        systemd_creds_available,
        credentials,
        findings,
        ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn mk_cred_dir(root: &Path) -> PathBuf {
        let cred_dir = root.join(".agents/credentials");
        fs::create_dir_all(&cred_dir).unwrap();
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
        cred_dir
    }

    #[test]
    fn audit_credentials_exempts_valid_bm25_memory_index_and_checkpoints() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let cred_dir = mk_cred_dir(ws);
        let cache_dir = ws.join(".agents/cache");
        fs::create_dir_all(&cache_dir).unwrap();
        let idx_path = cache_dir.join("memory_index.json");
        let valid_idx = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-11-25T13:00:00Z",
            "doc_count": 1,
            "avg_doc_len": 4.0,
            "docs": {
                ".agents/memory/security.md": {
                    "rel_path": ".agents/memory/security.md",
                    "mtime_secs": 1700000000u64,
                    "size_bytes": 120u64,
                    "sha256": "abc123",
                    "meta": {
                        "id": "security",
                        "title": "Sudoers NOPASSWD 与 Quadlet Secret 策略",
                        "type": "procedural",
                        "status": "active",
                        "scope": "security",
                        "created_at": "2026-11-25T13:00:00Z",
                        "updated_at": "2026-11-25T13:00:00Z",
                        "tags": ["secret", "nopasswd"],
                        "sources": ["cli:agent-memory"],
                        "confidence": 0.9
                    },
                    "doc_len": 4,
                    "terms": {
                        "secret": 3,
                        "nopasswd": 2,
                        "token": 7
                    },
                    "line_count": 12
                }
            }
        });
        fs::write(&idx_path, serde_json::to_string_pretty(&valid_idx).unwrap()).unwrap();

        // Checkpoints snapshot with a kv pattern must also be skipped like migrate::audit_workspace
        let cp_dir = ws.join(".agents/checkpoints/cp-001");
        fs::create_dir_all(&cp_dir).unwrap();
        let synthetic_pass = ["snap", "pass", "123"].concat();
        fs::write(
            cp_dir.join("legacy.txt"),
            format!("{}={synthetic_pass}\n", ["pass", "word"].concat()),
        )
        .unwrap();

        let report = audit_credentials(&cred_dir, &[ws.to_path_buf()], None);
        assert!(
            report.ok,
            "expected valid BM25 memory_index.json + checkpoints skip to pass; findings: {:?}",
            report.findings
        );
        assert!(report.findings.is_empty());
    }

    #[test]
    fn audit_credentials_flags_tampered_bm25_memory_index() {
        let tmp = TempDir::new().unwrap();
        let ws = tmp.path();
        let cred_dir = mk_cred_dir(ws);
        let cache_dir = ws.join(".agents/cache");
        fs::create_dir_all(&cache_dir).unwrap();
        let idx_path = cache_dir.join("memory_index.json");

        let runtime_tok = format!("{}_{}", ["gh", "p"].concat(), "T".repeat(24));
        let tampered_terms = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-11-25T13:00:00Z",
            "doc_count": 1,
            "avg_doc_len": 2.0,
            "docs": {
                ".agents/memory/security.md": {
                    "rel_path": ".agents/memory/security.md",
                    "mtime_secs": 1700000000u64,
                    "size_bytes": 50u64,
                    "sha256": "abc",
                    "meta": {
                        "id": "security",
                        "title": format!("leaked {runtime_tok}"),
                        "type": "procedural",
                        "status": "active",
                        "scope": "security",
                        "created_at": "2026-11-25T13:00:00Z",
                        "updated_at": "2026-11-25T13:00:00Z",
                        "tags": [],
                        "sources": ["cli:agent-memory"],
                        "confidence": 0.9
                    },
                    "doc_len": 2,
                    "terms": {
                        "secret": "not_an_integer_secret"
                    },
                    "line_count": 5
                }
            }
        });
        fs::write(
            &idx_path,
            serde_json::to_string_pretty(&tampered_terms).unwrap(),
        )
        .unwrap();

        let report = audit_credentials(&cred_dir, &[ws.to_path_buf()], None);
        assert!(!report.ok);
        assert!(report.findings.iter().any(|f| f.code == "SECRET_LEAK"));
    }
}
