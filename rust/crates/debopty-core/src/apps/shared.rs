use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::config::{ConfigStore, DEFAULT_CONFIG_PATH};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::network::{self, HttpClient};
use crate::security::SecurityContext;
use crate::sys::{self, CmdOutput, CommandRunner, PasswdEntry};
use crate::template::TemplateEngine;

const DIAG_TARGET: &str = "debopty::apps::shared";
const CERTS_GROUP: &str = "debopti-certs";
const CERTS_DIR_ABS: &str = "/etc/ferron/certs";
const SOT_KNOWN_USERS_ABS: &str = "/etc/debopti/sot_known_users";
const PODMAN_MAINTAINER_FISH_FILES: &[&str] = &[
    "debopti_apppod.fish",
    "debopti_appctl.fish",
    "debopti_applog.fish",
    "debopti_appshell.fish",
];

fn prefixed_path(root: &Path, abs: &str) -> PathBuf {
    let clean = abs.trim_start_matches('/');
    if root.as_os_str().is_empty() || root == Path::new("/") {
        PathBuf::from("/").join(clean)
    } else {
        root.join(clean)
    }
}

/// Shared execution context for all native Rust app deployment and management modules.
#[derive(Debug, Clone)]
pub struct AppContext {
    pub root_prefix: PathBuf,
    pub is_cn_region: bool,
    pub non_interactive: bool,
    pub no_sys: bool,
    pub config_path: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub arch_override: Option<String>,
    pub sudo_user_override: Option<String>,
    pub passwd_override: Option<PathBuf>,
    pub euid_override: Option<u32>,
}

impl Default for AppContext {
    fn default() -> Self {
        Self {
            root_prefix: PathBuf::from("/"),
            is_cn_region: false,
            non_interactive: false,
            no_sys: false,
            config_path: PathBuf::from(DEFAULT_CONFIG_PATH),
            template_dir: None,
            arch_override: None,
            sudo_user_override: None,
            passwd_override: None,
            euid_override: None,
        }
    }
}

impl AppContext {
    /// Construct an `AppContext` scoped under `root` for unprivileged tests or `--root`.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let config_path = prefixed_path(&root, DEFAULT_CONFIG_PATH);
        let passwd_override = Some(prefixed_path(&root, "/etc/passwd"));
        Self {
            root_prefix: root,
            is_cn_region: false,
            non_interactive: false,
            no_sys: false,
            config_path,
            template_dir: None,
            arch_override: None,
            sudo_user_override: None,
            passwd_override,
            euid_override: None,
        }
    }

    pub fn is_custom_root(&self) -> bool {
        !self.root_prefix.as_os_str().is_empty() && self.root_prefix != Path::new("/")
    }

    pub fn path(&self, abs: &str) -> PathBuf {
        prefixed_path(&self.root_prefix, abs)
    }

    pub fn effective_config_path(&self) -> PathBuf {
        if self.is_custom_root() && self.config_path == Path::new(DEFAULT_CONFIG_PATH) {
            self.path(DEFAULT_CONFIG_PATH)
        } else {
            self.config_path.clone()
        }
    }

    pub fn template_engine(&self) -> TemplateEngine {
        TemplateEngine::new(self.template_dir.clone()).with_root_prefix(&self.root_prefix)
    }

    pub fn config_store(&self) -> ConfigStore {
        ConfigStore::new(self.effective_config_path())
    }

    pub fn security_context(&self) -> SecurityContext {
        let mut sec = if self.is_custom_root() {
            SecurityContext::with_root(&self.root_prefix)
        } else {
            SecurityContext::default()
        };
        sec.no_sys = self.no_sys;
        sec.non_interactive = self.non_interactive;
        sec.config_path = self.effective_config_path();
        sec.template_dir = self.template_dir.clone();
        sec.binary_path = env::current_exe().ok();
        sec
    }

    pub fn arch(&self) -> String {
        self.arch_override
            .clone()
            .unwrap_or_else(|| std::env::consts::ARCH.to_string())
    }

    pub fn euid(&self) -> u32 {
        self.euid_override.unwrap_or_else(sys::current_euid)
    }

    pub fn passwd_entries(&self) -> Vec<PasswdEntry> {
        let passwd_path = self
            .passwd_override
            .clone()
            .unwrap_or_else(|| self.path("/etc/passwd"));
        fs::read_to_string(passwd_path)
            .map(|text| sys::parse_passwd(&text))
            .unwrap_or_default()
    }

    pub fn sot_user(&self) -> String {
        sys::sot_user(&self.passwd_entries())
    }

    pub fn initial_user(&self) -> String {
        let sudo_env = self
            .sudo_user_override
            .clone()
            .or_else(|| env::var("SUDO_USER").ok());
        sys::initial_user(sudo_env.as_deref())
    }

    pub fn all_real_users(&self) -> Vec<String> {
        let entries = self.passwd_entries();
        sys::all_real_users_with_dir_check(&entries, |p| self.path(&p.to_string_lossy()).is_dir())
    }

    /// Resolve a user's home directory strictly from `/etc/passwd` without shell `eval`.
    pub fn user_home(&self, username: &str) -> PathBuf {
        let raw_home = self
            .passwd_entries()
            .into_iter()
            .find(|e| e.name == username)
            .map(|e| e.home)
            .unwrap_or_else(|| {
                if username == "root" {
                    PathBuf::from("/root")
                } else {
                    PathBuf::from(format!("/home/{username}"))
                }
            });
        if self.is_custom_root() {
            self.path(&raw_home.to_string_lossy())
        } else {
            raw_home
        }
    }

    /// Check whether command `cmd` is installed under standard system bin directories
    /// (falling back to `which`/`PATH` on real root when `!no_sys`).
    pub fn is_cmd_present(&self, cmd: &str, runner: &dyn CommandRunner) -> bool {
        if cmd.is_empty() || cmd.contains('/') || cmd.contains('\0') {
            return false;
        }
        for dir in ["/usr/local/bin", "/usr/bin", "/usr/sbin", "/bin"] {
            let candidate = self.path(&format!("{dir}/{cmd}"));
            if candidate.is_file()
                && fs::metadata(&candidate)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
            {
                return true;
            }
        }
        if !self.no_sys && !self.is_custom_root() {
            if let Ok(out) = runner.run("which", &[cmd], Duration::from_secs(5))
                && out.success()
                && !out.stdout.trim().is_empty()
            {
                return true;
            }
            if let Some(paths) = env::var_os("PATH") {
                for dir in env::split_paths(&paths) {
                    let candidate = dir.join(cmd);
                    if candidate.is_file()
                        && fs::metadata(&candidate)
                            .map(|m| m.permissions().mode() & 0o111 != 0)
                            .unwrap_or(false)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Return `true` if CN region mode is enabled directly or persisted in `ConfigStore`.
    pub fn effective_is_cn(&self) -> bool {
        if self.is_cn_region {
            return true;
        }
        self.config_store()
            .get("IS_CN_REGION")
            .ok()
            .flatten()
            .is_some_and(|v| v.trim() == "true")
    }
}

/// Structured result report emitted by every native Rust app operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppReport {
    pub app: String,
    pub action: String,
    pub changed: bool,
    pub summary: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preserved_user_custom: Vec<String>,
}

impl AppReport {
    pub fn new(
        app: impl Into<String>,
        action: impl Into<String>,
        changed: bool,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            app: app.into(),
            action: action.into(),
            changed,
            summary: summary.into(),
            details: BTreeMap::new(),
            warnings: Vec::new(),
            preserved_user_custom: Vec::new(),
        }
    }

    pub fn with_detail(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.details.insert(k.into(), v.into());
        self
    }

    pub fn with_warning(mut self, msg: impl Into<String>) -> Self {
        self.warnings.push(msg.into());
        self
    }

    pub fn with_preserved(mut self, msg: impl Into<String>) -> Self {
        self.preserved_user_custom.push(msg.into());
        self
    }
}

/// Emit an informational progress line via `diag::log_event` (routed to the active TUI log sink when set).
pub fn emit_progress(target: &str, msg: &str) {
    diag::log_event(LogLevel::Info, target, None, msg, None, &[]);
}

/// Reject unsafe identifiers containing path separators, traversal sequences, control chars, or leading `-`.
pub fn validate_safe_name(raw: &str, label: &str) -> Result<()> {
    if raw.is_empty()
        || raw.starts_with('-')
        || raw.contains('/')
        || raw.contains('\\')
        || raw.contains("..")
        || raw.contains('\0')
        || raw.contains('\r')
        || raw.contains('\n')
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid {label}: '{raw}'"
        )));
    }
    Ok(())
}

/// Create a non-login system user if absent. Returns `Ok(true)` if newly created.
pub fn create_system_user(
    ctx: &AppContext,
    username: &str,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    validate_safe_name(username, "username")?;
    if ctx.no_sys {
        diag::log_event(
            LogLevel::Debug,
            DIAG_TARGET,
            None,
            "skip create_system_user in no_sys mode",
            None,
            &[("user", username)],
        );
        return Ok(false);
    }

    if let Ok(out) = runner.run("id", &["-u", username], Duration::from_secs(10))
        && out.success()
    {
        return Ok(false);
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "creating system user",
        None,
        &[("user", username)],
    );
    let out = runner.run(
        "useradd",
        &["-r", "-s", "/usr/sbin/nologin", username],
        Duration::from_secs(30),
    )?;
    if !out.success() {
        return Err(CoreError::App {
            app: "shared".to_string(),
            message: format!(
                "useradd failed for '{username}' (code {}): {}",
                out.code,
                out.stderr.trim()
            ),
        });
    }
    Ok(true)
}

/// Remove a system user if present. Returns `Ok(true)` if removed.
pub fn remove_system_user(
    ctx: &AppContext,
    username: &str,
    remove_home: bool,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    validate_safe_name(username, "username")?;
    if ctx.no_sys {
        return Ok(false);
    }

    let exists = runner
        .run("id", &["-u", username], Duration::from_secs(10))
        .map(|o| o.success())
        .unwrap_or(false);
    if !exists {
        return Ok(false);
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "removing system user",
        None,
        &[("user", username)],
    );
    let args: Vec<&str> = if remove_home {
        vec!["-r", username]
    } else {
        vec![username]
    };
    let out = runner.run("userdel", &args, Duration::from_secs(30))?;
    if !out.success() {
        return Err(CoreError::App {
            app: "shared".to_string(),
            message: format!(
                "userdel failed for '{username}' (code {}): {}",
                out.code,
                out.stderr.trim()
            ),
        });
    }
    Ok(true)
}

/// Render and deploy a systemd service unit file (`0644`), optionally reloading, enabling, and restarting it.
pub fn deploy_systemd_service(
    ctx: &AppContext,
    svc_name: &str,
    template_rel: &str,
    vars: &BTreeMap<String, String>,
    enable_and_start: bool,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    validate_safe_name(svc_name, "systemd service name")?;
    let svc_file = ctx.path(&format!("/etc/systemd/system/{svc_name}.service"));
    ctx.template_engine()
        .render_to_file(template_rel, &svc_file, vars, Some(0o644))?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "deployed systemd unit",
        None,
        &[("service", svc_name), ("path", &svc_file.to_string_lossy())],
    );

    if !ctx.no_sys {
        let reload = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30))?;
        if !reload.success() {
            return Err(CoreError::App {
                app: svc_name.to_string(),
                message: format!(
                    "systemctl daemon-reload failed (code {}): {}",
                    reload.code,
                    reload.stderr.trim()
                ),
            });
        }
        if enable_and_start {
            let _ = runner.run("systemctl", &["enable", svc_name], Duration::from_secs(30));
            let restart =
                runner.run("systemctl", &["restart", svc_name], Duration::from_secs(60))?;
            if !restart.success() {
                return Err(CoreError::App {
                    app: svc_name.to_string(),
                    message: format!(
                        "systemctl restart {svc_name} failed (code {}): {}",
                        restart.code,
                        restart.stderr.trim()
                    ),
                });
            }
        }
    }

    Ok(svc_file)
}

/// Render and deploy a systemd drop-in override (`security.conf`, mode `0644`), reloading and restarting if active.
pub fn inject_service_override(
    ctx: &AppContext,
    svc_name: &str,
    template_rel: &str,
    vars: &BTreeMap<String, String>,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    validate_safe_name(svc_name, "systemd service name")?;
    let override_file = ctx.path(&format!(
        "/etc/systemd/system/{svc_name}.service.d/security.conf"
    ));
    ctx.template_engine()
        .render_to_file(template_rel, &override_file, vars, Some(0o644))?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "injected systemd security override",
        None,
        &[
            ("service", svc_name),
            ("path", &override_file.to_string_lossy()),
        ],
    );

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
        if svc_name.ends_with('@') {
            let pattern = format!("{svc_name}*");
            if let Ok(list_out) = runner.run(
                "systemctl",
                &[
                    "list-units",
                    "--type=service",
                    "--state=active",
                    "--plain",
                    "--no-legend",
                    &pattern,
                ],
                Duration::from_secs(15),
            ) && list_out.success()
            {
                for line in list_out.stdout.lines() {
                    if let Some(unit) = line.split_whitespace().next()
                        && unit.starts_with(svc_name)
                    {
                        let _ =
                            runner.run("systemctl", &["restart", unit], Duration::from_secs(60));
                    }
                }
            }
        } else {
            let is_active = runner
                .run(
                    "systemctl",
                    &["is-active", "--quiet", svc_name],
                    Duration::from_secs(15),
                )
                .map(|o| o.success())
                .unwrap_or(false);
            if is_active {
                let _ = runner.run("systemctl", &["restart", svc_name], Duration::from_secs(60));
            }
        }
    }

    Ok(override_file)
}

/// Stop, disable, and remove a systemd service unit and its drop-in override directory.
pub fn remove_systemd_service(
    ctx: &AppContext,
    svc_name: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    validate_safe_name(svc_name, "systemd service name")?;
    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["stop", svc_name], Duration::from_secs(30));
        let _ = runner.run("systemctl", &["disable", svc_name], Duration::from_secs(30));
    }

    let svc_file = ctx.path(&format!("/etc/systemd/system/{svc_name}.service"));
    if svc_file.exists() || svc_file.symlink_metadata().is_ok() {
        let _ = fs::remove_file(&svc_file);
    }
    let override_dir = ctx.path(&format!("/etc/systemd/system/{svc_name}.service.d"));
    if override_dir.exists() {
        let _ = fs::remove_dir_all(&override_dir);
    }

    if !ctx.no_sys {
        let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(30));
    }

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "removed systemd service",
        None,
        &[("service", svc_name)],
    );
    Ok(())
}

fn validate_env_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.contains('=')
        || key.contains(['\r', '\n', '\0'])
        || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid environment variable key: '{key}'"
        )));
    }
    Ok(())
}

/// Atomically set or update `KEY="VAL"` in `/etc/environment` (mode `0644`), preserving all unrelated entries.
pub fn set_system_env(ctx: &AppContext, key: &str, val: &str) -> Result<()> {
    validate_env_key(key)?;
    if val.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid control characters in value for '{key}'"
        )));
    }
    let env_file = ctx.path("/etc/environment");
    let existing = if env_file.is_file() {
        fs::read_to_string(&env_file)?
    } else {
        String::new()
    };

    let prefix = format!("{key}=");
    let replacement = format!("{key}=\"{val}\"");
    let mut updated = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if line.starts_with(&prefix) {
            if !updated {
                out_lines.push(replacement.clone());
                updated = true;
            }
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !updated {
        out_lines.push(replacement);
    }

    let mut content = out_lines.join("\n");
    content.push('\n');
    sys::write_file_atomic(&env_file, content.as_bytes(), Some(0o644))?;
    diag::log_event(
        LogLevel::Debug,
        DIAG_TARGET,
        None,
        "updated /etc/environment key",
        None,
        &[("key", key)],
    );
    Ok(())
}

/// Atomically remove `KEY=...` from `/etc/environment` (mode `0644`), preserving all unrelated entries.
pub fn remove_system_env(ctx: &AppContext, key: &str) -> Result<()> {
    validate_env_key(key)?;
    let env_file = ctx.path("/etc/environment");
    if !env_file.is_file() {
        return Ok(());
    }
    let existing = fs::read_to_string(&env_file)?;
    let prefix = format!("{key}=");
    let out_lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.starts_with(&prefix))
        .collect();
    let mut content = out_lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    sys::write_file_atomic(&env_file, content.as_bytes(), Some(0o644))?;
    Ok(())
}

/// Best-effort `chown` using `<owner>:` (resolves primary group safely even when `group != user`).
pub fn best_effort_chown(
    ctx: &AppContext,
    owner: &str,
    path: &Path,
    recursive: bool,
    runner: &dyn CommandRunner,
) {
    if ctx.no_sys || owner.is_empty() {
        return;
    }
    let spec = format!("{owner}:");
    let p = path.to_string_lossy();
    if recursive {
        let _ = runner.run("chown", &["-R", &spec, &p], Duration::from_secs(15));
    } else {
        let _ = runner.run("chown", &[&spec, &p], Duration::from_secs(15));
    }
}

/// Safely quote a literal value for Fish shell (`'...'` escaping `\` and `'`).
fn fish_single_quote(val: &str) -> String {
    let mut out = String::with_capacity(val.len() + 2);
    out.push('\'');
    for ch in val.chars() {
        match ch {
            '\\' | '\'' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('\'');
    out
}

/// Safely quote a PATH entry for Fish shell while preserving a leading `$HOME` expansion.
fn fish_quote_path(target_path: &str) -> String {
    if let Some(rest) = target_path.strip_prefix("$HOME")
        && (rest.is_empty() || rest.starts_with('/'))
    {
        let mut out = String::with_capacity(target_path.len() + 2);
        out.push_str("\"$HOME");
        for ch in rest.chars() {
            match ch {
                '\\' | '"' | '$' => {
                    out.push('\\');
                    out.push(ch);
                }
                _ => out.push(ch),
            }
        }
        out.push('"');
        out
    } else {
        fish_single_quote(target_path)
    }
}

/// Atomically install an executable binary from `src` to `dest` (`0755`) via sibling temp file + `rename()`,
/// preventing `ETXTBSY` when upgrading an actively running executable.
pub fn install_binary_atomic_from_file(src: &Path, dest: &Path) -> Result<()> {
    let parent = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut reader = fs::File::open(src)?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    std::io::copy(&mut reader, tmp.as_file_mut())?;
    tmp.as_file().sync_all()?;
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o755))?;
    tmp.persist(dest).map_err(|e| CoreError::Io(e.error))?;
    Ok(())
}

/// Atomically set `set -gx KEY 'VAL'` in SOT user's `debopti_vars.fish` (mode `0644`).
pub fn update_fish_env(
    ctx: &AppContext,
    key: &str,
    val: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    validate_env_key(key)?;
    if val.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid control characters in Fish env value for '{key}'"
        )));
    }
    if !ctx.is_cmd_present("fish", runner) {
        return Ok(());
    }
    let sot_user = ctx.sot_user();
    let user_home = ctx.user_home(&sot_user);
    if !user_home.is_dir() {
        return Ok(());
    }

    let conf_d = user_home.join(".config/fish/conf.d");
    let env_file = conf_d.join("debopti_vars.fish");
    let existing = if env_file.is_file() {
        fs::read_to_string(&env_file)?
    } else {
        String::new()
    };

    let marker = format!("set -gx {key} ");
    let quoted_val = fish_single_quote(val);
    let replacement = format!("set -gx {key} {quoted_val}");
    let mut updated = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if line.contains(&marker) {
            if !updated {
                out_lines.push(replacement.clone());
                updated = true;
            }
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !updated {
        out_lines.push(replacement);
    }

    let mut content = out_lines.join("\n");
    content.push('\n');
    sys::write_file_atomic(&env_file, content.as_bytes(), Some(0o644))?;
    best_effort_chown(ctx, &sot_user, &env_file, false, runner);
    Ok(())
}

/// Atomically remove `set -gx KEY ...` from SOT user's `debopti_vars.fish` (mode `0644`).
pub fn remove_fish_env(ctx: &AppContext, key: &str, runner: &dyn CommandRunner) -> Result<()> {
    validate_env_key(key)?;
    if !ctx.is_cmd_present("fish", runner) {
        return Ok(());
    }
    let sot_user = ctx.sot_user();
    let env_file = ctx
        .user_home(&sot_user)
        .join(".config/fish/conf.d/debopti_vars.fish");
    if !env_file.is_file() {
        return Ok(());
    }
    let existing = fs::read_to_string(&env_file)?;
    let marker = format!("set -gx {key} ");
    let out_lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.contains(&marker))
        .collect();
    let mut content = out_lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    sys::write_file_atomic(&env_file, content.as_bytes(), Some(0o644))?;
    best_effort_chown(ctx, &sot_user, &env_file, false, runner);
    Ok(())
}

fn detect_fish_version(runner: &dyn CommandRunner) -> String {
    runner
        .run("fish", &["--version"], Duration::from_secs(5))
        .ok()
        .filter(|o| o.success())
        .and_then(|o| {
            o.stdout
                .split_whitespace()
                .nth(2)
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| "0.0".to_string())
}

/// Atomically add `target_path` to SOT user's `debopti_path.fish` (`fish_add_path` on Fish >= 3.2, legacy fallback otherwise).
pub fn update_fish_path(
    ctx: &AppContext,
    target_path: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if target_path.is_empty() || target_path.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Fish PATH entry: '{target_path}'"
        )));
    }
    if !ctx.is_cmd_present("fish", runner) {
        return Ok(());
    }
    let sot_user = ctx.sot_user();
    let user_home = ctx.user_home(&sot_user);
    if !user_home.is_dir() {
        return Ok(());
    }

    let path_file = user_home.join(".config/fish/conf.d/debopti_path.fish");
    let existing = if path_file.is_file() {
        fs::read_to_string(&path_file)?
    } else {
        String::new()
    };

    let quoted_path = fish_quote_path(target_path);
    let fish_ver = detect_fish_version(runner);
    let desired_line = if compare_dot_version_ge(&fish_ver, "3.2") {
        format!("fish_add_path {quoted_path}")
    } else {
        format!("if not contains {quoted_path} $PATH; set -gx PATH $PATH {quoted_path}; end")
    };

    let already_present = existing.lines().any(|line| {
        line.contains(&format!("fish_add_path {quoted_path}"))
            || line.contains(&format!("contains {quoted_path} $PATH"))
            || line.contains(&format!("fish_add_path {target_path}"))
            || line.contains(&format!("contains {target_path} $PATH"))
    });
    if already_present {
        return Ok(());
    }

    let mut out_lines: Vec<String> = existing.lines().map(String::from).collect();
    out_lines.push(desired_line);
    let mut content = out_lines.join("\n");
    content.push('\n');
    sys::write_file_atomic(&path_file, content.as_bytes(), Some(0o644))?;
    best_effort_chown(ctx, &sot_user, &path_file, false, runner);
    Ok(())
}

/// Atomically remove any line referencing `target_path` from SOT user's `debopti_path.fish`.
pub fn remove_fish_path(
    ctx: &AppContext,
    target_path: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if target_path.is_empty() || target_path.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(format!(
            "invalid Fish PATH entry: '{target_path}'"
        )));
    }
    if !ctx.is_cmd_present("fish", runner) {
        return Ok(());
    }
    let sot_user = ctx.sot_user();
    let path_file = ctx
        .user_home(&sot_user)
        .join(".config/fish/conf.d/debopti_path.fish");
    if !path_file.is_file() {
        return Ok(());
    }
    let existing = fs::read_to_string(&path_file)?;
    let quoted_path = fish_quote_path(target_path);
    let out_lines: Vec<&str> = existing
        .lines()
        .filter(|line| !line.contains(target_path) && !line.contains(&quoted_path))
        .collect();
    let mut content = out_lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    sys::write_file_atomic(&path_file, content.as_bytes(), Some(0o644))?;
    best_effort_chown(ctx, &sot_user, &path_file, false, runner);
    Ok(())
}

/// Detect the Debian suite codename (`/etc/os-release` -> `lsb_release -cs` -> `/etc/debian_version`).
pub fn detect_debian_codename(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<String> {
    let os_release = ctx.path("/etc/os-release");
    if let Ok(text) = fs::read_to_string(&os_release) {
        for line in text.lines() {
            if let Some(rest) = line.trim().strip_prefix("VERSION_CODENAME=") {
                let val = rest.trim().trim_matches('"').trim_matches('\'').trim();
                if !val.is_empty() {
                    return Ok(val.to_string());
                }
            }
        }
    }

    if let Ok(out) = runner.run("lsb_release", &["-cs"], Duration::from_secs(10))
        && out.success()
    {
        let codename = out.stdout.trim();
        if !codename.is_empty() && !codename.eq_ignore_ascii_case("n/a") {
            return Ok(codename.to_string());
        }
    }

    let deb_ver = ctx.path("/etc/debian_version");
    if let Some(major) = sys::detect_debian_major(&deb_ver) {
        let mapped = match major {
            10 => Some("buster"),
            11 => Some("bullseye"),
            12 => Some("bookworm"),
            13 => Some("trixie"),
            14 => Some("forky"),
            _ => None,
        };
        if let Some(name) = mapped {
            return Ok(name.to_string());
        }
    }

    Err(CoreError::App {
        app: "shared".to_string(),
        message: "unable to detect Debian codename from os-release, lsb_release, or debian_version"
            .to_string(),
    })
}

/// Download a PGP key, dearmor into `keyring_abs`, and write `/etc/apt/sources.list.d/{repo_name}.list` (`0644`).
pub fn install_apt_keyring_and_repo(
    ctx: &AppContext,
    repo_name: &str,
    pgp_url: &str,
    keyring_abs: &str,
    repo_line: &str,
    http: &dyn HttpClient,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    validate_safe_name(repo_name, "APT repo name")?;
    let tmp_pgp = NamedTempFile::new()?;
    let tmp_path = tmp_pgp.path().to_path_buf();

    network::download_with_fallback(&tmp_path, pgp_url, ctx.effective_is_cn(), http)?;

    let keyring_path = ctx.path(keyring_abs);
    if let Some(parent) = keyring_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let keyring_str = keyring_path.to_string_lossy().into_owned();
    let tmp_str = tmp_path.to_string_lossy().into_owned();
    if !ctx.no_sys {
        let gpg_out = runner.run(
            "gpg",
            &["--dearmor", "--yes", "-o", &keyring_str, &tmp_str],
            Duration::from_secs(30),
        )?;
        if !gpg_out.success() {
            return Err(CoreError::App {
                app: repo_name.to_string(),
                message: format!(
                    "gpg --dearmor failed for '{repo_name}' (code {}): {}",
                    gpg_out.code,
                    gpg_out.stderr.trim()
                ),
            });
        }
        if keyring_path.is_file() {
            fs::set_permissions(&keyring_path, fs::Permissions::from_mode(0o644))?;
        }
    }

    let list_path = ctx.path(&format!("/etc/apt/sources.list.d/{repo_name}.list"));
    let content = format!("{}\n", repo_line.trim_end());
    sys::write_file_atomic(&list_path, content.as_bytes(), Some(0o644))?;

    diag::log_event(
        LogLevel::Info,
        DIAG_TARGET,
        None,
        "installed APT keyring and repository list",
        None,
        &[("repo", repo_name), ("list", &list_path.to_string_lossy())],
    );
    Ok(list_path)
}

fn ensure_certs_group(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<()> {
    if ctx.no_sys {
        return Ok(());
    }
    let exists = runner
        .run("getent", &["group", CERTS_GROUP], Duration::from_secs(10))
        .map(|o| o.success())
        .unwrap_or(false);
    if !exists {
        let out = runner.run(
            "groupadd",
            &["--system", CERTS_GROUP],
            Duration::from_secs(15),
        )?;
        if !out.success() {
            return Err(CoreError::App {
                app: "shared".to_string(),
                message: format!(
                    "groupadd {CERTS_GROUP} failed (code {}): {}",
                    out.code,
                    out.stderr.trim()
                ),
            });
        }
    }
    Ok(())
}

/// Add `user` to the shared `debopti-certs` group if the user exists and is not yet a member.
pub fn add_user_to_certs_group(
    ctx: &AppContext,
    user: &str,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    validate_safe_name(user, "username")?;
    if ctx.no_sys {
        return Ok(false);
    }
    let user_exists = runner
        .run("id", &["-u", user], Duration::from_secs(10))
        .map(|o| o.success())
        .unwrap_or(false);
    if !user_exists {
        return Ok(false);
    }

    ensure_certs_group(ctx, runner)?;

    let already_member = runner
        .run("id", &["-nG", user], Duration::from_secs(10))
        .ok()
        .filter(|o| o.success())
        .is_some_and(|o| o.stdout.split_whitespace().any(|g| g == CERTS_GROUP));
    if already_member {
        return Ok(false);
    }

    let out = runner.run(
        "usermod",
        &["-aG", CERTS_GROUP, user],
        Duration::from_secs(15),
    )?;
    if !out.success() {
        return Err(CoreError::App {
            app: "shared".to_string(),
            message: format!(
                "usermod -aG {CERTS_GROUP} {user} failed (code {}): {}",
                out.code,
                out.stderr.trim()
            ),
        });
    }
    Ok(true)
}

/// Prepare `/etc/ferron/certs` (`0750`, group `debopti-certs`) and add `ferron` user if present.
pub fn prepare_shared_certs_dir(ctx: &AppContext, runner: &dyn CommandRunner) -> Result<PathBuf> {
    ensure_certs_group(ctx, runner)?;
    let certs_dir = ctx.path(CERTS_DIR_ABS);
    fs::create_dir_all(&certs_dir)?;
    fs::set_permissions(&certs_dir, fs::Permissions::from_mode(0o750))?;

    if add_user_to_certs_group(ctx, "ferron", runner)? && !ctx.no_sys {
        let _ = runner.run(
            "systemctl",
            &["try-restart", "ferron"],
            Duration::from_secs(30),
        );
    }

    if !ctx.no_sys && !ctx.is_custom_root() {
        let dir_str = certs_dir.to_string_lossy();
        let _ = runner.run(
            "chown",
            &[&format!("root:{CERTS_GROUP}"), &dir_str],
            Duration::from_secs(10),
        );
    }
    Ok(certs_dir)
}

fn is_podman_maintainer_fish_file(filename: &str) -> bool {
    PODMAN_MAINTAINER_FISH_FILES.contains(&filename)
}

fn is_podman_maintainer_user(user: &str, sot_user: &str) -> bool {
    user == "root" || user == sot_user
}

fn apply_traverse_perms(path: &Path) {
    if let Ok(meta) = fs::metadata(path) {
        let mode = meta.permissions().mode() | 0o005;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
}

fn apply_shared_readonly_tree(root: &Path) {
    if !root.is_dir() {
        return;
    }
    for entry in walkdir::WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let p = entry.path();
        if let Ok(meta) = fs::symlink_metadata(p) {
            if meta.file_type().is_symlink() {
                continue;
            }
            let cur = meta.permissions().mode() & 0o7777;
            let next = if meta.is_dir() {
                (cur | 0o005) & !0o002
            } else {
                (cur | 0o004) & !0o002
            };
            let _ = fs::set_permissions(p, fs::Permissions::from_mode(next));
        }
    }
}

fn replace_with_symlink(target: &Path, dest: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() || meta.is_file() {
            fs::remove_file(dest)?;
        } else if meta.is_dir() {
            fs::remove_dir_all(dest)?;
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink(target, dest)?;
    Ok(())
}

/// Replace `dest` with a symlink to `target`, EXCEPT when `dest` is an existing non-symlink
/// directory containing user files or a non-symlink file (`U8`: never destroy user custom configs).
/// Returns `Ok(true)` if symlinked, `Ok(false)` if preserved untouched.
fn link_sot_preserving_user_custom(target: &Path, dest: &Path) -> Result<bool> {
    if let Ok(meta) = fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() {
            fs::remove_file(dest)?;
        } else if meta.is_dir() {
            let has_entries = fs::read_dir(dest)
                .ok()
                .and_then(|mut rd| rd.next())
                .is_some();
            if has_entries {
                diag::log_event(
                    LogLevel::Info,
                    DIAG_TARGET,
                    None,
                    "preserving existing non-SOT user config directory (U8)",
                    None,
                    &[("path", &dest.to_string_lossy())],
                );
                return Ok(false);
            }
            fs::remove_dir(dest)?;
        } else {
            diag::log_event(
                LogLevel::Info,
                DIAG_TARGET,
                None,
                "preserving existing non-SOT user config file (U8)",
                None,
                &[("path", &dest.to_string_lossy())],
            );
            return Ok(false);
        }
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    symlink(target, dest)?;
    Ok(true)
}

fn link_user_fish_hybrid(
    ctx: &AppContext,
    user: &str,
    u_home: &Path,
    sot_home: &Path,
    sot_user: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    let sot_fish = sot_home.join(".config/fish");
    if !sot_fish.is_dir() {
        return Ok(());
    }
    let user_fish = u_home.join(".config/fish");
    let user_conf_d = user_fish.join("conf.d");
    let sot_conf_d = sot_fish.join("conf.d");

    if fs::symlink_metadata(&user_fish)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        fs::remove_file(&user_fish)?;
    }
    fs::create_dir_all(&user_conf_d)?;
    fs::create_dir_all(user_fish.join("functions"))?;
    fs::create_dir_all(user_fish.join("conf.d.local"))?;

    for sub in ["conf.d", "functions", "completions", "themes"] {
        let sub_path = user_fish.join(sub);
        if fs::symlink_metadata(&sub_path)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            fs::remove_file(&sub_path)?;
            fs::create_dir_all(&sub_path)?;
        }
    }

    if sot_conf_d.is_dir() {
        for entry in fs::read_dir(&sot_conf_d)? {
            let entry = entry?;
            let src = entry.path();
            if !src.is_file() {
                continue;
            }
            let Some(base) = src.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !base.ends_with(".fish") {
                continue;
            }
            let target = user_conf_d.join(base);
            if is_podman_maintainer_fish_file(base) && !is_podman_maintainer_user(user, sot_user) {
                if fs::symlink_metadata(&target).is_ok() {
                    let _ = fs::remove_file(&target);
                }
                continue;
            }
            // U8: preserve non-symlink user-created files in conf.d
            if let Ok(meta) = fs::symlink_metadata(&target)
                && !meta.file_type().is_symlink()
            {
                continue;
            }
            replace_with_symlink(&src, &target)?;
        }
    }

    let mut vars = BTreeMap::new();
    vars.insert(
        "SOT_FISH".to_string(),
        sot_fish.to_string_lossy().into_owned(),
    );
    let bootstrap_path = user_conf_d.join("00-debopti-sot-bootstrap.fish");
    ctx.template_engine().render_to_file(
        "templates/apps/devops/fish_sot_bootstrap.fish",
        &bootstrap_path,
        &vars,
        Some(0o644),
    )?;

    let sot_plugins = sot_fish.join("fish_plugins");
    let user_plugins = user_fish.join("fish_plugins");
    if sot_plugins.is_file() && !user_plugins.exists() {
        fs::copy(&sot_plugins, &user_plugins)?;
    }

    let fish_vars = user_fish.join("fish_variables");
    if !fish_vars.exists() {
        sys::write_file_atomic(&fish_vars, b"", Some(0o600))?;
    } else {
        fs::set_permissions(&fish_vars, fs::Permissions::from_mode(0o600))?;
    }

    fs::create_dir_all(u_home.join(".local/share/fish"))?;
    best_effort_chown(ctx, user, &user_fish, true, runner);
    best_effort_chown(ctx, user, &u_home.join(".local/share/fish"), true, runner);
    Ok(())
}

/// Synchronize Fish (hybrid links), Micro, Yazi, and Starship SOT links for all real users.
pub fn sync_devops_sot_links(
    ctx: &AppContext,
    quiet: bool,
    runner: &dyn CommandRunner,
) -> Result<bool> {
    let sot_user = ctx.sot_user();
    let sot_home = ctx.user_home(&sot_user);
    if !sot_home.is_dir() {
        return Ok(false);
    }

    let sot_fish = sot_home.join(".config/fish");
    let sot_micro = sot_home.join(".config/micro");
    let sot_yazi = sot_home.join(".config/yazi");

    let has_fish = sot_fish.is_dir() && ctx.is_cmd_present("fish", runner);
    let has_micro = sot_micro.is_dir();
    let has_yazi = sot_yazi.is_dir();
    if !has_fish && !has_micro && !has_yazi {
        return Ok(false);
    }

    if !quiet {
        diag::log_event(
            LogLevel::Info,
            DIAG_TARGET,
            None,
            "syncing DevOps SOT readonly symlinks",
            None,
            &[("sot_user", &sot_user)],
        );
    }

    apply_traverse_perms(&sot_home);
    apply_traverse_perms(&sot_home.join(".config"));
    if has_fish {
        apply_shared_readonly_tree(&sot_fish);
    }
    if has_micro {
        apply_shared_readonly_tree(&sot_micro);
    }
    if has_yazi {
        apply_shared_readonly_tree(&sot_yazi);
    }

    let starship_path = ctx.path("/etc/starship.toml");
    let all_users = ctx.all_real_users();
    for u in &all_users {
        let u_home = ctx.user_home(u);
        if !u_home.is_dir() {
            continue;
        }
        let u_config = u_home.join(".config");
        fs::create_dir_all(&u_config)?;

        if has_fish && starship_path.is_file() {
            let link_target = if ctx.is_custom_root() {
                starship_path.clone()
            } else {
                PathBuf::from("/etc/starship.toml")
            };
            replace_with_symlink(&link_target, &u_config.join("starship.toml"))?;
        }

        if u == &sot_user {
            continue;
        }

        if has_fish {
            link_user_fish_hybrid(ctx, u, &u_home, &sot_home, &sot_user, runner)?;
        }
        if has_micro {
            let _ = link_sot_preserving_user_custom(&sot_micro, &u_config.join("micro"))?;
        }
        if has_yazi {
            let _ = link_sot_preserving_user_custom(&sot_yazi, &u_config.join("yazi"))?;
        }
    }

    Ok(true)
}

/// Detect newly added real users against `/etc/debopti/sot_known_users` and sync DevOps SOT links.
pub fn maybe_sync_devops_sot_on_startup(
    ctx: &AppContext,
    runner: &dyn CommandRunner,
) -> Result<Vec<String>> {
    let all_users = ctx.all_real_users();
    let known_path = ctx.path(SOT_KNOWN_USERS_ABS);
    let known: BTreeSet<String> = fs::read_to_string(&known_path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    let new_users: Vec<String> = all_users
        .iter()
        .filter(|u| !known.contains(*u))
        .cloned()
        .collect();

    if !new_users.is_empty() {
        diag::log_event(
            LogLevel::Info,
            DIAG_TARGET,
            None,
            "detected new real users for DevOps SOT sync",
            None,
            &[("users", &new_users.join(","))],
        );
    }

    let _ = sync_devops_sot_links(ctx, true, runner)?;
    let mut rendered = all_users.join("\n");
    if !rendered.is_empty() {
        rendered.push('\n');
    }
    sys::write_file_atomic(&known_path, rendered.as_bytes(), Some(0o644))?;
    Ok(new_users)
}

/// Extract a `.tar.gz` archive into `dest_dir` via `tar -xzf`.
pub fn extract_tar_gz(archive: &Path, dest_dir: &Path, runner: &dyn CommandRunner) -> Result<()> {
    fs::create_dir_all(dest_dir)?;
    let archive_str = archive.to_string_lossy();
    let dest_str = dest_dir.to_string_lossy();
    let out = runner.run(
        "tar",
        &["-xzf", &archive_str, "-C", &dest_str],
        Duration::from_secs(120),
    )?;
    if !out.success() {
        return Err(CoreError::App {
            app: "shared".to_string(),
            message: format!(
                "tar extraction failed for '{}' (code {}): {}",
                archive.display(),
                out.code,
                out.stderr.trim()
            ),
        });
    }
    Ok(())
}

/// Extract a `.zip` archive into `dest_dir` via `unzip -qo`.
pub fn extract_zip(archive: &Path, dest_dir: &Path, runner: &dyn CommandRunner) -> Result<()> {
    fs::create_dir_all(dest_dir)?;
    let archive_str = archive.to_string_lossy();
    let dest_str = dest_dir.to_string_lossy();
    let out = runner.run(
        "unzip",
        &["-qo", &archive_str, "-d", &dest_str],
        Duration::from_secs(120),
    )?;
    if !out.success() {
        return Err(CoreError::App {
            app: "shared".to_string(),
            message: format!(
                "unzip extraction failed for '{}' (code {}): {}",
                archive.display(),
                out.code,
                out.stderr.trim()
            ),
        });
    }
    Ok(())
}

/// Execute `prog` with `args` and `env` as `target_user` (using `sudo -H -u <user> env ...` when non-root).
pub fn run_as_user(
    _ctx: &AppContext,
    target_user: &str,
    prog: &str,
    args: &[&str],
    env: &[(&str, &str)],
    timeout: Duration,
    runner: &dyn CommandRunner,
) -> Result<CmdOutput> {
    validate_safe_name(target_user, "target user")?;
    if target_user == "root" {
        return runner.run_with_env(prog, args, env, timeout);
    }

    let mut owned_args: Vec<String> =
        vec!["-H".to_string(), "-u".to_string(), target_user.to_string()];
    if !env.is_empty() {
        owned_args.push("env".to_string());
        for &(k, v) in env {
            owned_args.push(format!("{k}={v}"));
        }
    }
    owned_args.push(prog.to_string());
    for &a in args {
        owned_args.push(a.to_string());
    }
    let arg_refs: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    runner.run("sudo", &arg_refs, timeout)
}

/// Pure-Rust RFC 4648 Base64 decoder (tolerates ASCII whitespace and optional missing `=` padding).
pub fn decode_base64(input: &str) -> Result<Vec<u8>> {
    let cleaned: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if cleaned.is_empty() {
        return Ok(Vec::new());
    }
    if cleaned.len() % 4 == 1 {
        return Err(CoreError::ValidationFailed(
            "invalid base64 input length".to_string(),
        ));
    }

    let mut out = Vec::with_capacity((cleaned.len() / 4) * 3 + 2);
    let mut i = 0;
    let mut saw_padding = false;
    while i < cleaned.len() {
        if saw_padding {
            return Err(CoreError::ValidationFailed(
                "unexpected base64 characters after padding".to_string(),
            ));
        }
        let rem = cleaned.len() - i;
        let chunk = &cleaned[i..i + rem.min(4)];

        let mut vals = [0u8; 4];
        let mut pad = 0usize;
        for (idx, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                if idx < 2 || (idx == 2 && chunk.len() == 4 && chunk[3] != b'=') {
                    return Err(CoreError::ValidationFailed(
                        "misplaced base64 padding '='".to_string(),
                    ));
                }
                pad += 1;
                saw_padding = true;
            } else if saw_padding {
                return Err(CoreError::ValidationFailed(
                    "invalid base64 character after '='".to_string(),
                ));
            } else {
                vals[idx] = decode_b64_char(b).ok_or_else(|| {
                    CoreError::ValidationFailed(format!("invalid base64 byte: 0x{b:02x}"))
                })?;
            }
        }

        let effective_len = chunk.len() - pad;
        match effective_len {
            2 => {
                out.push((vals[0] << 2) | (vals[1] >> 4));
            }
            3 => {
                out.push((vals[0] << 2) | (vals[1] >> 4));
                out.push((vals[1] << 4) | (vals[2] >> 2));
            }
            4 => {
                out.push((vals[0] << 2) | (vals[1] >> 4));
                out.push((vals[1] << 4) | (vals[2] >> 2));
                out.push((vals[2] << 6) | vals[3]);
            }
            _ => {
                return Err(CoreError::ValidationFailed(
                    "invalid base64 quantum".to_string(),
                ));
            }
        }
        i += chunk.len();
    }

    Ok(out)
}

fn decode_b64_char(b: u8) -> Option<u8> {
    match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'+' | b'-' => Some(62),
        b'/' | b'_' => Some(63),
        _ => None,
    }
}

fn parse_dot_version_parts(raw: &str) -> Option<Vec<u64>> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    if trimmed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    for seg in trimmed.split('.') {
        let digits: String = seg.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            if parts.is_empty() {
                return None;
            }
            break;
        }
        parts.push(digits.parse::<u64>().ok()?);
    }
    if parts.is_empty() { None } else { Some(parts) }
}

/// Pure-Rust dot-separated numeric version comparison (`current >= minimum`).
pub fn compare_dot_version_ge(current: &str, minimum: &str) -> bool {
    let (Some(cur), Some(min)) = (
        parse_dot_version_parts(current),
        parse_dot_version_parts(minimum),
    ) else {
        return false;
    };
    let len = cur.len().max(min.len());
    for i in 0..len {
        let c = *cur.get(i).unwrap_or(&0);
        let m = *min.get(i).unwrap_or(&0);
        if c != m {
            return c > m;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    use crate::network::MockHttpClient;
    use crate::sys::MockCommandRunner;

    fn ok_out(stdout: &str) -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: stdout.to_string(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn make_executable(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn embedded_engine_includes_and_renders_all_8_new_p3_templates() {
        let engine = TemplateEngine::new(None);
        let list = engine.list();
        let new_templates = [
            "templates/apps/easytier/easytier@.service.override.conf",
            "templates/apps/tailscale/tailscaled.service.override.conf",
            "templates/apps/warp/warp-svc.service.override.conf",
            "templates/apps/warp/usque.service",
            "templates/apps/golang/caddy.service",
            "templates/apps/golang/Caddyfile",
            "templates/apps/golang/index.html",
            "templates/apps/devops/yazi_bashrc_snippet.sh",
        ];
        let empty = BTreeMap::new();
        for rel in new_templates {
            assert!(list.contains(&rel), "missing embedded template: {rel}");
            let rendered = engine.render_to_string(rel, &empty).unwrap();
            assert!(!rendered.trim().is_empty(), "empty template: {rel}");
            assert!(
                !rendered.contains("{{"),
                "unexpected leftover placeholder in {rel}: {rendered}"
            );
        }
    }

    #[test]
    fn systemd_deploy_override_and_remove_under_temp_root() {
        let tmp = TempDir::new().unwrap();
        let ctx = AppContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();
        runner.on_prog("systemctl", ok_out(""));

        let svc_path = deploy_systemd_service(
            &ctx,
            "usque",
            "templates/apps/warp/usque.service",
            &BTreeMap::new(),
            true,
            &runner,
        )
        .unwrap();
        assert!(svc_path.is_file());
        assert_eq!(
            fs::metadata(&svc_path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(
            fs::read_to_string(&svc_path)
                .unwrap()
                .contains("ExecStart=/opt/usque/usque")
        );

        let override_path = inject_service_override(
            &ctx,
            "warp-svc",
            "templates/apps/warp/warp-svc.service.override.conf",
            &BTreeMap::new(),
            &runner,
        )
        .unwrap();
        assert!(override_path.is_file());
        assert_eq!(
            fs::metadata(&override_path).unwrap().permissions().mode() & 0o777,
            0o644
        );

        let invs = runner.invocations();
        assert!(
            invs.iter()
                .any(|(p, a)| p == "systemctl" && a == &["daemon-reload"])
        );
        assert!(
            invs.iter()
                .any(|(p, a)| p == "systemctl" && a == &["enable", "usque"])
        );
        assert!(
            invs.iter()
                .any(|(p, a)| p == "systemctl" && a == &["restart", "usque"])
        );

        remove_systemd_service(&ctx, "usque", &runner).unwrap();
        assert!(!svc_path.exists());
    }

    #[test]
    fn system_env_and_fish_env_path_preserve_unrelated_user_entries() {
        let tmp = TempDir::new().unwrap();
        let ctx = AppContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();
        runner.on_exact("fish", &["--version"], ok_out("fish, version 3.7.1\n"));

        // Setup fake fish binary & /etc/passwd with alice as SOT user
        make_executable(&ctx.path("/usr/bin/fish"));
        let alice_home = ctx.path("/home/alice");
        fs::create_dir_all(&alice_home).unwrap();
        sys::write_file_atomic(
            &ctx.path("/etc/passwd"),
            b"root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000::/home/alice:/usr/bin/fish\n",
            Some(0o644),
        )
        .unwrap();

        // Pre-populate /etc/environment with user custom entry
        sys::write_file_atomic(
            &ctx.path("/etc/environment"),
            b"# custom comment\nUSER_CUSTOM=\"keep_me\"\nFOO=\"old\"\n",
            Some(0o644),
        )
        .unwrap();

        set_system_env(&ctx, "FOO", "new").unwrap();
        set_system_env(&ctx, "BAR", "baz").unwrap();
        let env_txt = fs::read_to_string(ctx.path("/etc/environment")).unwrap();
        assert!(env_txt.contains("# custom comment"));
        assert!(env_txt.contains("USER_CUSTOM=\"keep_me\""));
        assert!(env_txt.contains("FOO=\"new\""));
        assert!(env_txt.contains("BAR=\"baz\""));

        remove_system_env(&ctx, "FOO").unwrap();
        let env_after = fs::read_to_string(ctx.path("/etc/environment")).unwrap();
        assert!(!env_after.contains("FOO="));
        assert!(env_after.contains("USER_CUSTOM=\"keep_me\""));
        assert!(env_after.contains("BAR=\"baz\""));

        // Fish env + path
        let vars_file = alice_home.join(".config/fish/conf.d/debopti_vars.fish");
        sys::write_file_atomic(&vars_file, b"set -gx CUSTOM_VAR keep\n", Some(0o644)).unwrap();
        update_fish_env(&ctx, "GOPROXY", "https://goproxy.cn,direct", &runner).unwrap();
        let vars_txt = fs::read_to_string(&vars_file).unwrap();
        assert!(vars_txt.contains("set -gx CUSTOM_VAR keep"));
        assert!(vars_txt.contains("set -gx GOPROXY 'https://goproxy.cn,direct'"));

        remove_fish_env(&ctx, "GOPROXY", &runner).unwrap();
        let vars_after = fs::read_to_string(&vars_file).unwrap();
        assert!(vars_after.contains("set -gx CUSTOM_VAR keep"));
        assert!(!vars_after.contains("GOPROXY"));

        update_fish_path(&ctx, "/usr/local/go/bin", &runner).unwrap();
        update_fish_path(&ctx, "/opt/easytier", &runner).unwrap();
        // Idempotent second call
        update_fish_path(&ctx, "/usr/local/go/bin", &runner).unwrap();
        let path_file = alice_home.join(".config/fish/conf.d/debopti_path.fish");
        let path_txt = fs::read_to_string(&path_file).unwrap();
        assert_eq!(
            path_txt
                .matches("fish_add_path '/usr/local/go/bin'")
                .count(),
            1
        );
        assert!(path_txt.contains("fish_add_path '/opt/easytier'"));

        remove_fish_path(&ctx, "/opt/easytier", &runner).unwrap();
        let path_after = fs::read_to_string(&path_file).unwrap();
        assert!(path_after.contains("fish_add_path '/usr/local/go/bin'"));
        assert!(!path_after.contains("/opt/easytier"));
    }

    #[test]
    fn sync_devops_sot_links_hybrid_fish_preserves_user_files_and_filters_podman_helpers() {
        let tmp = TempDir::new().unwrap();
        let ctx = AppContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();

        make_executable(&ctx.path("/usr/bin/fish"));
        let root_home = ctx.path("/root");
        let alice_home = ctx.path("/home/alice");
        let bob_home = ctx.path("/home/bob");
        fs::create_dir_all(alice_home.join(".config/fish/conf.d")).unwrap();
        fs::create_dir_all(alice_home.join(".config/micro")).unwrap();
        fs::create_dir_all(&root_home).unwrap();
        fs::create_dir_all(bob_home.join(".config/fish/conf.d")).unwrap();

        sys::write_file_atomic(
            &ctx.path("/etc/passwd"),
            concat!(
                "root:x:0:0:root:/root:/bin/bash\n",
                "alice:x:1000:1000::/home/alice:/usr/bin/fish\n",
                "bob:x:1001:1001::/home/bob:/usr/bin/fish\n"
            )
            .as_bytes(),
            Some(0o644),
        )
        .unwrap();

        // SOT managed files in alice's conf.d
        sys::write_file_atomic(
            &alice_home.join(".config/fish/conf.d/debopti_vars.fish"),
            b"set -gx EDITOR micro\n",
            Some(0o644),
        )
        .unwrap();
        sys::write_file_atomic(
            &alice_home.join(".config/fish/conf.d/debopti_apppod.fish"),
            b"# podman maintainer only\n",
            Some(0o644),
        )
        .unwrap();
        sys::write_file_atomic(
            &alice_home.join(".config/fish/fish_plugins"),
            b"jorgebucaran/fisher\n",
            Some(0o644),
        )
        .unwrap();
        sys::write_file_atomic(
            &ctx.path("/etc/starship.toml"),
            b"format = '$all'\n",
            Some(0o644),
        )
        .unwrap();

        // Bob has a non-symlink user custom file with the same name as debopti_vars.fish
        let bob_custom = bob_home.join(".config/fish/conf.d/debopti_vars.fish");
        sys::write_file_atomic(&bob_custom, b"# bob custom\n", Some(0o644)).unwrap();

        let new_users = maybe_sync_devops_sot_on_startup(&ctx, &runner).unwrap();
        assert_eq!(new_users, vec!["root", "alice", "bob"]);

        // Second startup detects 0 new users
        let second = maybe_sync_devops_sot_on_startup(&ctx, &runner).unwrap();
        assert!(second.is_empty());

        // Root gets debopti_apppod.fish (maintainer), Bob does NOT
        assert!(
            fs::symlink_metadata(root_home.join(".config/fish/conf.d/debopti_apppod.fish"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            !bob_home
                .join(".config/fish/conf.d/debopti_apppod.fish")
                .exists()
        );

        // Bob's custom non-symlink file is preserved (U8)
        assert!(
            !fs::symlink_metadata(&bob_custom)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&bob_custom).unwrap(), "# bob custom\n");

        // Bob gets conf.d.local/, fish_variables (0600), bootstrap, micro symlink, starship symlink
        assert!(bob_home.join(".config/fish/conf.d.local").is_dir());
        assert_eq!(
            fs::metadata(bob_home.join(".config/fish/fish_variables"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert!(
            bob_home
                .join(".config/fish/conf.d/00-debopti-sot-bootstrap.fish")
                .is_file()
        );
        assert!(
            fs::symlink_metadata(bob_home.join(".config/micro"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(bob_home.join(".config/starship.toml"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn detect_debian_codename_os_release_lsb_and_version_fallbacks() {
        let tmp = TempDir::new().unwrap();
        let ctx = AppContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();

        // 1. /etc/os-release priority
        sys::write_file_atomic(
            &ctx.path("/etc/os-release"),
            b"ID=debian\nVERSION_CODENAME=bookworm\n",
            Some(0o644),
        )
        .unwrap();
        assert_eq!(detect_debian_codename(&ctx, &runner).unwrap(), "bookworm");

        // 2. lsb_release -cs fallback
        fs::remove_file(ctx.path("/etc/os-release")).unwrap();
        runner.on_exact("lsb_release", &["-cs"], ok_out("trixie\n"));
        assert_eq!(detect_debian_codename(&ctx, &runner).unwrap(), "trixie");

        // 3. /etc/debian_version fallback (10..=14)
        let failing_runner = MockCommandRunner::new();
        for (ver, expected) in [
            ("10.13\n", "buster"),
            ("11.9\n", "bullseye"),
            ("12.7\n", "bookworm"),
            ("13.0\n", "trixie"),
            ("14.0\n", "forky"),
        ] {
            sys::write_file_atomic(
                &ctx.path("/etc/debian_version"),
                ver.as_bytes(),
                Some(0o644),
            )
            .unwrap();
            assert_eq!(
                detect_debian_codename(&ctx, &failing_runner).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn install_apt_keyring_and_repo_downloads_dearmors_and_writes_list() {
        let tmp = TempDir::new().unwrap();
        let ctx = AppContext::with_root(tmp.path());
        let http = MockHttpClient::new();
        http.on_download(
            "https://example.test/key.pgp",
            200,
            b"PGP_KEY_BYTES".to_vec(),
        );
        let runner = MockCommandRunner::new();
        runner.on_prog("gpg", ok_out(""));

        let list_path = install_apt_keyring_and_repo(
            &ctx,
            "ferron",
            "https://example.test/key.pgp",
            "/usr/share/keyrings/ferron-keyring.gpg",
            "deb [signed-by=/usr/share/keyrings/ferron-keyring.gpg] https://deb.ferron.sh bookworm main",
            &http,
            &runner,
        )
        .unwrap();

        assert!(list_path.is_file());
        assert!(
            fs::read_to_string(&list_path)
                .unwrap()
                .contains("https://deb.ferron.sh bookworm main")
        );
        assert!(
            runner
                .invocations()
                .iter()
                .any(|(p, a)| p == "gpg" && a.contains(&"--dearmor".to_string()))
        );
    }

    #[test]
    fn decode_base64_and_compare_dot_version_ge_edge_cases() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("SGVsbG8=").unwrap(), b"Hello");
        assert_eq!(decode_base64("SGVsbG8").unwrap(), b"Hello");
        assert_eq!(decode_base64("  YWJj\nZA== ").unwrap(), b"abcd");
        assert!(decode_base64("A").is_err());
        assert!(decode_base64("SGVs***=").is_err());
        assert!(decode_base64("=AAA").is_err());

        assert!(compare_dot_version_ge("0.53", "0.53.0"));
        assert!(compare_dot_version_ge("0.53.0", "0.53"));
        assert!(compare_dot_version_ge("1.2.0", "0.53"));
        assert!(compare_dot_version_ge("v5.1.0", "5"));
        assert!(!compare_dot_version_ge("0.44.1", "0.53"));
        assert!(!compare_dot_version_ge("invalid", "0.53"));
    }

    #[test]
    fn validate_safe_name_rejects_traversal_and_control_chars() {
        assert!(validate_safe_name("easytier@", "svc").is_ok());
        assert!(validate_safe_name("warp-svc", "svc").is_ok());
        assert!(validate_safe_name("", "svc").is_err());
        assert!(validate_safe_name("-rf", "svc").is_err());
        assert!(validate_safe_name("../etc", "svc").is_err());
        assert!(validate_safe_name("a/b", "svc").is_err());
        assert!(validate_safe_name("a\nb", "svc").is_err());
    }

    #[test]
    fn emit_progress_routes_info_log_to_thread_log_sink() {
        use std::sync::{Arc, Mutex};

        let captured = Arc::new(Mutex::new(Vec::<String>::new()));
        let captured_cb = Arc::clone(&captured);
        let _guard = diag::install_thread_log_sink(
            Arc::new(move |line| {
                captured_cb.lock().unwrap().push(line.to_string());
            }),
            LogLevel::Info,
        );

        emit_progress("debopty::apps::test", "installing step 1/2");

        let lines = captured.lock().unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("[INFO] debopty::apps::test: installing step 1/2"));
    }
}
