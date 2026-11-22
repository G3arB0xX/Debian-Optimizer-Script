use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use regex::Regex;

use crate::config::ConfigStore;
use crate::error::{CoreError, Result};
use crate::security::f2b_sync;
use crate::security::fail2ban;
use crate::security::firewall;
use crate::security::{ensure_trailing_newline, write_file_atomic};
use crate::sys::{self, CommandRunner};

pub use crate::security::SecurityContext;

const HARDENING_DROPIN_PATH: &str = "/etc/ssh/sshd_config.d/00-debopty-hardening.conf";

fn ed25519_pubkey_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^ssh-ed25519 [A-Za-z0-9+/=]{40,}(?: [^\r\n\x00]*)?$")
            .expect("valid ed25519 pubkey regex")
    })
}

fn sshd_match_block_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^\s*match\s+").expect("valid sshd match regex"))
}

fn sshd_include_conf_d_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*include\s+.*sshd_config\.d/\*\.conf").expect("valid sshd include re")
    })
}

/// Parse `OpenSSH_<major>.<minor>` from `sshd -V` or `ssh -V` output.
pub fn parse_openssh_version(output: &str) -> Option<(u32, u32)> {
    let idx = output.find("OpenSSH_")?;
    let rest = &output[idx + "OpenSSH_".len()..];
    let mut parts = rest.split('.');
    let major_str: String = parts
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let minor_str: String = parts
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if major_str.is_empty() || minor_str.is_empty() {
        return None;
    }
    Some((major_str.parse().ok()?, minor_str.parse().ok()?))
}

fn detect_debian_major_under_root(root_prefix: &Path) -> Option<u32> {
    let prefix = |rel: &str| {
        if root_prefix.as_os_str().is_empty() || root_prefix == Path::new("/") {
            PathBuf::from("/").join(rel)
        } else {
            root_prefix.join(rel)
        }
    };

    if let Ok(os_rel) = fs::read_to_string(prefix("etc/os-release")) {
        for line in os_rel.lines() {
            let trimmed = line.trim();
            if let Some(val) = trimmed.strip_prefix("VERSION_ID=") {
                let clean = val.trim().trim_matches('"').trim_matches('\'');
                let digits: String = clean.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(ver) = digits.parse::<u32>() {
                    return Some(ver);
                }
            }
        }
    }

    if let Ok(deb_ver) = fs::read_to_string(prefix("etc/debian_version")) {
        let lower = deb_ver.trim().to_ascii_lowercase();
        let digits: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(ver) = digits.parse::<u32>() {
            return Some(ver);
        }
        if lower.starts_with("buster") {
            return Some(10);
        }
        if lower.starts_with("bullseye") {
            return Some(11);
        }
        if lower.starts_with("bookworm") {
            return Some(12);
        }
        if lower.starts_with("trixie") {
            return Some(13);
        }
    }

    None
}

/// Select the OpenSSH-compatible keyboard-interactive directive:
/// - OpenSSH `>= 8.7` (Debian 12+): `KbdInteractiveAuthentication`
/// - OpenSSH `< 8.7` (Debian 10 = 7.9, Debian 11 = 8.4): `ChallengeResponseAuthentication`
///
/// Note: OpenSSH `< 8.7` rejects `KbdInteractiveAuthentication` in `sshd -t`, and `sshd -V`
/// may exit non-zero on `< 8.7` while still emitting `OpenSSH_X.Y` on stderr, so both stderr
/// and stdout are inspected regardless of exit status, with `ssh -V` and `/etc/os-release`
/// fallbacks.
pub(crate) fn preferred_kbd_interactive_directive(
    root_prefix: &Path,
    no_sys: bool,
    runner: &dyn CommandRunner,
) -> &'static str {
    if !no_sys {
        for (prog, args) in [("sshd", &["-V"][..]), ("ssh", &["-V"][..])] {
            if let Ok(out) = runner.run(prog, args, Duration::from_secs(5)) {
                let ver = parse_openssh_version(&out.stderr)
                    .or_else(|| parse_openssh_version(&out.stdout));
                if let Some((major, minor)) = ver {
                    return if (major, minor) >= (8, 7) {
                        "KbdInteractiveAuthentication"
                    } else {
                        "ChallengeResponseAuthentication"
                    };
                }
            }
        }
    }

    if let Some(deb_major) = detect_debian_major_under_root(root_prefix) {
        return if deb_major >= 12 {
            "KbdInteractiveAuthentication"
        } else {
            "ChallengeResponseAuthentication"
        };
    }

    "KbdInteractiveAuthentication"
}

/// Interactive prompt abstraction for SSH hardening, enabling 100% deterministic unit tests.
pub trait SshPrompter {
    /// Ask whether to perform SSH hardening (`[Y/n]`, default `true`).
    fn confirm_harden(&self) -> Result<bool>;
    /// Ask whether to reuse an existing normal user (`(y/n)`).
    fn confirm_use_existing_user(&self, username: &str) -> Result<bool>;
    /// Prompt for a new username (empty string selects `"admin"`).
    fn prompt_new_username(&self) -> Result<String>;
    /// Prompt for user's Ed25519 SSH public key.
    fn prompt_ssh_pubkey(&self) -> Result<String>;
    /// Ask whether the user successfully logged in via the new SSH port (`(y/n)`).
    fn confirm_new_port_login(&self, user: &str, new_port: u16) -> Result<bool>;
    /// Optional password supplied by a non-interactive prompter for newly created users.
    fn non_interactive_password(&self) -> Option<String> {
        None
    }
    /// Whether this prompter reads from an interactive TTY (`true` for `StdioPrompter`).
    fn requires_tty(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Default)]
pub struct NonInteractivePrompter {
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_pubkey: Option<String>,
}

impl SshPrompter for NonInteractivePrompter {
    fn confirm_harden(&self) -> Result<bool> {
        Ok(true)
    }

    fn confirm_use_existing_user(&self, _username: &str) -> Result<bool> {
        Ok(true)
    }

    fn prompt_new_username(&self) -> Result<String> {
        Ok(self
            .username
            .clone()
            .or_else(|| {
                std::env::var("TEST_USERNAME")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_else(|| "admin".to_string()))
    }

    fn prompt_ssh_pubkey(&self) -> Result<String> {
        Ok(self
            .ssh_pubkey
            .clone()
            .or_else(|| {
                std::env::var("TEST_SSH_PUBKEY")
                    .ok()
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or_default())
    }

    fn confirm_new_port_login(&self, _user: &str, _new_port: u16) -> Result<bool> {
        Ok(true)
    }

    fn non_interactive_password(&self) -> Option<String> {
        self.password.clone().filter(|s| !s.is_empty()).or_else(|| {
            std::env::var("TEST_USER_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty())
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StdioPrompter;

impl StdioPrompter {
    fn read_line(prompt: &str) -> Result<String> {
        if !io::stdin().is_terminal() {
            return Err(CoreError::ValidationFailed(
                "SSH hardening requires an interactive TTY (or explicit --non-interactive with --pubkey)"
                    .to_string(),
            ));
        }
        print!("{prompt}");
        io::stdout().flush()?;
        let mut line = String::new();
        let stdin = io::stdin();
        let n = stdin.lock().read_line(&mut line)?;
        if n == 0 {
            return Err(CoreError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected EOF reading from stdin",
            )));
        }
        Ok(line.trim().to_string())
    }
}

impl SshPrompter for StdioPrompter {
    fn confirm_harden(&self) -> Result<bool> {
        let ans = Self::read_line(
            "是否进行 SSH 安全加固与普通用户创建？(创建 sudo 用户、锁定 root、加固 SSH) [Y/n]: ",
        )?;
        Ok(ans.is_empty() || ans.eq_ignore_ascii_case("y"))
    }

    fn confirm_use_existing_user(&self, username: &str) -> Result<bool> {
        let ans = Self::read_line(&format!("是否使用现有用户 '{username}' 进行加固？(y/n): "))?;
        Ok(ans.eq_ignore_ascii_case("y"))
    }

    fn prompt_new_username(&self) -> Result<String> {
        let ans = Self::read_line("请输入要创建的新用户名 (默认: admin): ")?;
        if ans.is_empty() {
            Ok("admin".to_string())
        } else {
            Ok(ans)
        }
    }

    fn prompt_ssh_pubkey(&self) -> Result<String> {
        Self::read_line("粘贴公钥: ")
    }

    fn confirm_new_port_login(&self, user: &str, new_port: u16) -> Result<bool> {
        let ans = Self::read_line(&format!(
            "您是否已成功通过新端口 ({user} @ port {new_port}) 登录？(y/n): "
        ))?;
        Ok(ans.eq_ignore_ascii_case("y"))
    }

    fn requires_tty(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshHardenReport {
    pub skipped: bool,
    pub user: Option<String>,
    pub old_port: u16,
    pub new_port: u16,
    pub has_socket: bool,
    pub locked_down: bool,
    pub rolled_back: bool,
}

/// Validate POSIX username matching `^[a-z][-a-z0-9]*$`.
pub fn is_valid_username(username: &str) -> bool {
    let mut bytes = username.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Validate a single-line OpenSSH Ed25519 public key strictly (rejects newlines, NULs, options, non-ed25519).
pub fn is_valid_ed25519_pubkey(key: &str) -> bool {
    let trimmed = key.trim();
    if trimmed.is_empty() || trimmed.contains(['\r', '\n', '\0']) {
        return false;
    }
    ed25519_pubkey_re().is_match(trimmed)
}

/// Generate a random high SSH port in `40000..=65534` without external dependencies.
pub fn random_high_port() -> u16 {
    let mut seed: u32 = 0;
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        let mut buf = [0u8; 4];
        if file.read_exact(&mut buf).is_ok() {
            seed = u32::from_ne_bytes(buf);
        }
    }
    if seed == 0 {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(17);
        seed = nanos ^ std::process::id().wrapping_mul(2_654_435_761);
    }
    40000 + (seed % 25535) as u16
}

/// Generate a random high SSH port in `40000..=65534`, avoiding `exclude` and actively bound local ports.
pub fn random_high_port_excluding(exclude: u16) -> u16 {
    for _ in 0..32 {
        let candidate = random_high_port();
        if candidate == exclude {
            continue;
        }
        match TcpListener::bind(("127.0.0.1", candidate)) {
            Ok(listener) => {
                drop(listener);
                return candidate;
            }
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => continue,
            Err(_) => return candidate,
        }
    }
    if exclude == 40000 { 40001 } else { 40000 }
}

fn user_in_group_file(group_content: &str, group_name: &str, username: &str) -> bool {
    for line in group_content.lines() {
        let mut fields = line.split(':');
        let Some(name) = fields.next() else {
            continue;
        };
        if name != group_name {
            continue;
        }
        if let Some(members) = fields.nth(2)
            && members.split(',').any(|m| m.trim() == username)
        {
            return true;
        }
    }
    false
}

fn ensure_user_sudo_membership(
    ctx: &SecurityContext,
    username: &str,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if ctx.no_sys {
        return Ok(());
    }

    let group_content = fs::read_to_string(ctx.path("/etc/group")).unwrap_or_default();
    if user_in_group_file(&group_content, "sudo", username)
        || user_in_group_file(&group_content, "wheel", username)
    {
        return Ok(());
    }

    if let Ok(out) = runner.run("id", &["-nG", username], Duration::from_secs(5))
        && out.success()
        && out
            .stdout
            .split_whitespace()
            .any(|g| g == "sudo" || g == "wheel")
    {
        return Ok(());
    }

    let target_group = if group_content.lines().any(|l| l.starts_with("sudo:")) {
        Some("sudo")
    } else if group_content.lines().any(|l| l.starts_with("wheel:")) {
        Some("wheel")
    } else {
        None
    };

    if let Some(grp) = target_group {
        let out = runner.run("usermod", &["-aG", grp, username], Duration::from_secs(10))?;
        if !out.success() {
            return Err(CoreError::Security(format!(
                "failed to grant {grp} membership to '{username}': {}",
                out.stderr.trim()
            )));
        }
        return Ok(());
    }

    if ctx.path(&format!("/etc/sudoers.d/{username}")).is_file() {
        return Ok(());
    }

    Err(CoreError::Security(format!(
        "cannot verify sudo/wheel privileges for '{username}'; refusing to proceed before locking root"
    )))
}

/// Update or insert a global OpenSSH directive in `sshd_config` strictly above the first `Match` block.
fn set_sshd_directive(path: &Path, key: &str, value: &str) -> Result<()> {
    if key.is_empty() || key.contains(['\r', '\n', '\0']) || value.contains(['\r', '\n', '\0']) {
        return Err(CoreError::InvalidConfig(format!(
            "invalid control characters in sshd_config directive '{key}'"
        )));
    }

    let existing = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let pattern = format!(r"(?i)^\s*#?\s*{}(?:\s*=\s*|\s+)", regex::escape(key));
    let key_re = Regex::new(&pattern)
        .map_err(|e| CoreError::InvalidConfig(format!("invalid sshd directive regex: {e}")))?;
    let replacement = format!("{key} {value}");

    let lines: Vec<&str> = existing.lines().collect();
    let match_idx = lines
        .iter()
        .position(|line| sshd_match_block_re().is_match(line))
        .unwrap_or(lines.len());

    let mut out_lines: Vec<String> = Vec::with_capacity(lines.len() + 1);
    let mut updated = false;

    for &line in &lines[..match_idx] {
        if key_re.is_match(line) {
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

    for &line in &lines[match_idx..] {
        out_lines.push(line.to_string());
    }

    let mut rendered = out_lines.join("\n");
    rendered.push('\n');
    write_file_atomic(path, rendered.as_bytes(), None)
}

fn sshd_has_conf_d_include(sshd_conf: &Path) -> bool {
    let Ok(content) = fs::read_to_string(sshd_conf) else {
        return false;
    };
    for line in content.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            continue;
        }
        if sshd_match_block_re().is_match(line) {
            break;
        }
        if sshd_include_conf_d_re().is_match(line) {
            return true;
        }
    }
    false
}

/// Detect an existing normal user (`uid >= 1000`, excluding `nobody`) or create a new sudo user.
pub fn detect_or_create_user(
    ctx: &SecurityContext,
    prompter: &dyn SshPrompter,
    runner: &dyn CommandRunner,
) -> Result<String> {
    let passwd_path = ctx.path("/etc/passwd");
    let entries = fs::read_to_string(&passwd_path)
        .map(|t| sys::parse_passwd(&t))
        .unwrap_or_default();

    if let Some(selected) = sys::normal_user(&entries)
        && (ctx.non_interactive || prompter.confirm_use_existing_user(&selected)?)
    {
        ensure_user_sudo_membership(ctx, &selected, runner)?;
        return Ok(selected);
    }

    let mut chosen: Option<String> = None;
    for _attempt in 0..3 {
        let raw = prompter.prompt_new_username()?;
        let candidate = if raw.trim().is_empty() {
            "admin".to_string()
        } else {
            raw.trim().to_string()
        };

        if !is_valid_username(&candidate) {
            if ctx.non_interactive {
                return Err(CoreError::ValidationFailed(format!(
                    "invalid username: {candidate}"
                )));
            }
            continue;
        }

        let exists_in_passwd = entries.iter().any(|e| e.name == candidate);
        let exists_in_sys = !ctx.no_sys
            && runner
                .run("id", &[&candidate], Duration::from_secs(5))
                .map(|o| o.success())
                .unwrap_or(false);

        if exists_in_passwd || exists_in_sys {
            if ctx.non_interactive {
                ensure_user_sudo_membership(ctx, &candidate, runner)?;
                return Ok(candidate);
            }
            continue;
        }

        chosen = Some(candidate);
        break;
    }

    let username = chosen.ok_or_else(|| {
        CoreError::Security("aborted user creation after 3 invalid attempts".to_string())
    })?;

    // Validate non-interactive password preconditions BEFORE mutating system accounts.
    let non_interactive_pass = if ctx.non_interactive {
        let maybe_pass = prompter.non_interactive_password().or_else(|| {
            ctx.allow_test_ephemeral_auth
                .then(|| "admin123".to_string())
        });
        if let Some(ref p) = maybe_pass
            && p.contains(['\r', '\n', '\0'])
        {
            return Err(CoreError::ValidationFailed(
                "password must not contain newline or NUL characters".to_string(),
            ));
        }
        if !ctx.no_sys && maybe_pass.is_none() {
            return Err(CoreError::ValidationFailed(
                "non-interactive user creation requires an explicit password (refusing default password without explicit test opt-in)"
                    .to_string(),
            ));
        }
        maybe_pass
    } else {
        None
    };

    if !ctx.no_sys {
        let out = runner.run(
            "useradd",
            &["-m", "-s", "/bin/bash", &username],
            Duration::from_secs(15),
        )?;
        if !out.success() {
            return Err(CoreError::Security(format!(
                "useradd failed for {username}: {}",
                out.stderr.trim()
            )));
        }

        if ctx.non_interactive {
            let pass = non_interactive_pass.expect("validated above");
            let chp = runner.run_with_stdin(
                "chpasswd",
                &[],
                &format!("{username}:{pass}\n"),
                Duration::from_secs(15),
            )?;
            if !chp.success() {
                return Err(CoreError::Security(format!(
                    "chpasswd failed for {username}: {}",
                    chp.stderr.trim()
                )));
            }
        } else {
            let code = runner.run_interactive("passwd", &[&username])?;
            if code != 0 {
                return Err(CoreError::Security(format!(
                    "passwd failed for {username} (exit code {code})"
                )));
            }
        }

        ensure_user_sudo_membership(ctx, &username, runner)?;
    }

    Ok(username)
}

/// Inject an Ed25519 SSH public key into the target user's `authorized_keys` (appending without wiping existing keys).
///
/// Looks up user home strictly via `sys::parse_passwd` (zero shell tilde evaluation).
pub fn setup_user_ssh_key(
    ctx: &SecurityContext,
    user: &str,
    prompter: &dyn SshPrompter,
    runner: &dyn CommandRunner,
) -> Result<PathBuf> {
    let entries = fs::read_to_string(ctx.path("/etc/passwd"))
        .map(|t| sys::parse_passwd(&t))
        .unwrap_or_default();

    let raw_home = entries
        .iter()
        .find(|e| e.name == user)
        .map(|e| e.home.clone())
        .unwrap_or_else(|| {
            if user == "root" {
                PathBuf::from("/root")
            } else {
                PathBuf::from(format!("/home/{user}"))
            }
        });

    let mut pub_key = prompter.prompt_ssh_pubkey()?.trim().to_string();

    if ctx.non_interactive {
        if pub_key.is_empty() {
            if !ctx.allow_test_ephemeral_auth {
                return Err(CoreError::ValidationFailed(
                    "non-interactive SSH hardening requires an explicit Ed25519 public key (refusing ephemeral test_key generation without explicit test opt-in)"
                        .to_string(),
                ));
            }
            let key_base = ctx.path("/opt/debopty/test_key");
            if let Some(parent) = key_base.parent() {
                fs::create_dir_all(parent)?;
            }
            let _ = fs::remove_file(&key_base);
            let pub_path = PathBuf::from(format!("{}.pub", key_base.display()));
            let _ = fs::remove_file(&pub_path);

            let key_str = key_base.to_string_lossy();
            let out = runner.run(
                "ssh-keygen",
                &["-t", "ed25519", "-N", "", "-f", &key_str],
                Duration::from_secs(15),
            )?;
            if !out.success() {
                return Err(CoreError::Security(format!(
                    "ssh-keygen failed: {}",
                    out.stderr.trim()
                )));
            }
            pub_key = fs::read_to_string(&pub_path)?.trim().to_string();
        }
        if !is_valid_ed25519_pubkey(&pub_key) {
            return Err(CoreError::ValidationFailed(
                "invalid single-line ssh-ed25519 public key".to_string(),
            ));
        }
    } else {
        let mut attempts = 0;
        while !is_valid_ed25519_pubkey(&pub_key) {
            attempts += 1;
            if attempts >= 5 {
                return Err(CoreError::ValidationFailed(
                    "valid single-line ssh-ed25519 public key required".to_string(),
                ));
            }
            pub_key = prompter.prompt_ssh_pubkey()?.trim().to_string();
        }
    }

    let user_home = ctx.path(&raw_home.to_string_lossy());
    let ssh_dir = user_home.join(".ssh");
    fs::create_dir_all(&ssh_dir)?;
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))?;

    let auth_keys = ssh_dir.join("authorized_keys");
    let new_key_blob = pub_key.split_whitespace().nth(1);
    let mut lines: Vec<String> = if auth_keys.is_file() {
        fs::read_to_string(&auth_keys)?
            .lines()
            .map(|l| l.trim_end_matches('\r').to_string())
            .filter(|l| !l.trim().is_empty())
            .collect()
    } else {
        Vec::new()
    };

    let already_present = lines.iter().any(|existing| {
        let trimmed = existing.trim();
        if trimmed == pub_key {
            return true;
        }
        if let Some(blob) = new_key_blob {
            let mut parts = trimmed.split_whitespace();
            return parts.next() == Some("ssh-ed25519") && parts.next() == Some(blob);
        }
        false
    });
    if !already_present {
        lines.push(pub_key);
    }

    let content = ensure_trailing_newline(lines.join("\n"));
    write_file_atomic(&auth_keys, content.as_bytes(), Some(0o600))?;

    if !ctx.no_sys {
        let owner = format!("{user}:");
        let ssh_dir_str = ssh_dir.to_string_lossy();
        let out = runner.run(
            "chown",
            &["-R", &owner, &ssh_dir_str],
            Duration::from_secs(10),
        )?;
        if !out.success() {
            return Err(CoreError::Security(format!(
                "chown failed for {}: {}",
                ssh_dir.display(),
                out.stderr.trim()
            )));
        }
    }

    Ok(auth_keys)
}

/// Apply new SSH listening port via `ssh.socket` override or `/etc/ssh/sshd_config`.
///
/// Validates `sshd -t` before restarting `ssh` and restores `sshd_config` on validation failure.
pub fn apply_ssh_port(
    ctx: &SecurityContext,
    port: u16,
    has_socket: bool,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if port == 0 {
        return Err(CoreError::ValidationFailed(
            "SSH port must be in 1..=65535".to_string(),
        ));
    }

    if has_socket {
        let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
        let prev_override = fs::read(&override_path).ok();
        let engine = ctx.template_engine();
        let mut vars = BTreeMap::new();
        vars.insert("PORT".to_string(), port.to_string());
        let rendered = ensure_trailing_newline(
            engine.render_to_string("templates/security/ssh.socket.override.conf", &vars)?,
        );
        write_file_atomic(&override_path, rendered.as_bytes(), Some(0o644))?;

        if !ctx.no_sys {
            let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
            let out = runner.run(
                "systemctl",
                &["restart", "ssh.socket"],
                Duration::from_secs(15),
            )?;
            if !out.success() {
                if let Some(bytes) = prev_override {
                    let _ = write_file_atomic(&override_path, &bytes, Some(0o644));
                } else {
                    let _ = fs::remove_file(&override_path);
                }
                let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
                return Err(CoreError::Security(format!(
                    "failed to restart ssh.socket: {}",
                    out.stderr.trim()
                )));
            }
        }
    } else {
        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        let prev_sshd = fs::read(&sshd_conf).ok();
        set_sshd_directive(&sshd_conf, "Port", &port.to_string())?;

        if !ctx.no_sys {
            let check = runner.run("sshd", &["-t"], Duration::from_secs(10))?;
            if !check.success() {
                if let Some(bytes) = prev_sshd {
                    let _ = write_file_atomic(&sshd_conf, &bytes, None);
                }
                return Err(CoreError::Security(format!(
                    "sshd -t validation failed before restart: {}",
                    check.stderr.trim()
                )));
            }
            let out = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15))?;
            if !out.success() {
                if let Some(bytes) = prev_sshd {
                    let _ = write_file_atomic(&sshd_conf, &bytes, None);
                }
                return Err(CoreError::Security(format!(
                    "failed to restart ssh: {}",
                    out.stderr.trim()
                )));
            }
        }
    }

    Ok(())
}

fn rollback_ssh_changes_with_snapshot(
    ctx: &SecurityContext,
    old_port: u16,
    has_socket: bool,
    new_port: u16,
    override_snapshot: Option<&[u8]>,
    sshd_snapshot: Option<&[u8]>,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if has_socket {
        let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
        if let Some(prev) = override_snapshot {
            write_file_atomic(&override_path, prev, Some(0o644))?;
        } else if old_port != 22 {
            let engine = ctx.template_engine();
            let mut vars = BTreeMap::new();
            vars.insert("PORT".to_string(), old_port.to_string());
            let rendered = ensure_trailing_newline(
                engine.render_to_string("templates/security/ssh.socket.override.conf", &vars)?,
            );
            write_file_atomic(&override_path, rendered.as_bytes(), Some(0o644))?;
        } else if override_path.exists() {
            let _ = fs::remove_file(&override_path);
        }
        if !ctx.no_sys {
            let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
            let _ = runner.run(
                "systemctl",
                &["restart", "ssh.socket"],
                Duration::from_secs(15),
            );
        }
    } else {
        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        if let Some(prev) = sshd_snapshot {
            write_file_atomic(&sshd_conf, prev, None)?;
        } else if sshd_conf.is_file() {
            let content = fs::read_to_string(&sshd_conf)?;
            let target = format!("Port {new_port}");
            let replacement = format!("Port {old_port}");
            let mut out_lines: Vec<String> = Vec::new();
            for line in content.lines() {
                if line.trim_start().eq_ignore_ascii_case(&target) {
                    out_lines.push(replacement.clone());
                } else {
                    out_lines.push(line.to_string());
                }
            }
            let mut rendered = out_lines.join("\n");
            rendered.push('\n');
            write_file_atomic(&sshd_conf, rendered.as_bytes(), None)?;
        }
        if !ctx.no_sys {
            let _ = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15));
        }
    }

    let nft_conf_dir = ctx.effective_nft_conf_dir();
    for legacy in [
        "SSH_Hardened_Port.nft",
        "SSH_Listen_Port.nft",
        "SSH_Access_Port.nft",
    ] {
        let p = nft_conf_dir.join(legacy);
        if p.exists() {
            let _ = fs::remove_file(p);
        }
    }

    firewall::update_ssh_firewall(ctx, old_port, runner)?;

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(())
}

/// Atomically roll back SSH port, socket override / `sshd_config`, firewall rule, and Fail2ban sync.
pub fn rollback_ssh_changes(
    ctx: &SecurityContext,
    old_port: u16,
    has_socket: bool,
    new_port: u16,
    runner: &dyn CommandRunner,
) -> Result<()> {
    rollback_ssh_changes_with_snapshot(ctx, old_port, has_socket, new_port, None, None, runner)
}

/// Disable root login & password authentication, lock `root`, and persist `SSH_HARDENED="true"`.
///
/// Writes a high-priority drop-in `/etc/ssh/sshd_config.d/00-debopty-hardening.conf` when
/// `sshd_config` includes `sshd_config.d/*.conf`, updates `sshd_config` globally above any
/// `Match` block, and validates `sshd -t` before restarting `ssh` or locking `root`.
pub fn lockdown_ssh_system(ctx: &SecurityContext, runner: &dyn CommandRunner) -> Result<()> {
    let sshd_conf = ctx.path("/etc/ssh/sshd_config");
    let dropin_path = ctx.path(HARDENING_DROPIN_PATH);
    let prev_sshd = fs::read(&sshd_conf).ok();
    let prev_dropin = fs::read(&dropin_path).ok();

    let restore_lockdown_files = |wrote_dropin: bool| {
        if let Some(ref bytes) = prev_sshd {
            let _ = write_file_atomic(&sshd_conf, bytes, None);
        }
        if wrote_dropin {
            if let Some(ref bytes) = prev_dropin {
                let _ = write_file_atomic(&dropin_path, bytes, Some(0o644));
            } else {
                let _ = fs::remove_file(&dropin_path);
            }
        }
    };

    let kbd_key = preferred_kbd_interactive_directive(&ctx.root_prefix, ctx.no_sys, runner);

    let wrote_dropin = sshd_has_conf_d_include(&sshd_conf);
    if wrote_dropin {
        let engine = ctx.template_engine();
        let mut vars = BTreeMap::new();
        vars.insert("PERMIT_ROOT_LOGIN".to_string(), "no".to_string());
        vars.insert("PASSWORD_AUTH".to_string(), "no".to_string());
        vars.insert("KBD_INTERACTIVE_KEY".to_string(), kbd_key.to_string());
        let dropin_content = ensure_trailing_newline(
            engine.render_to_string("templates/security/00-debopty-hardening.conf", &vars)?,
        );
        write_file_atomic(&dropin_path, dropin_content.as_bytes(), Some(0o644))?;
    }

    set_sshd_directive(&sshd_conf, "PermitRootLogin", "no")?;
    set_sshd_directive(&sshd_conf, "PasswordAuthentication", "no")?;
    set_sshd_directive(&sshd_conf, kbd_key, "no")?;
    set_sshd_directive(&sshd_conf, "PubkeyAuthentication", "yes")?;

    if !ctx.no_sys {
        let check = runner.run("sshd", &["-t"], Duration::from_secs(10))?;
        if !check.success() {
            restore_lockdown_files(wrote_dropin);
            return Err(CoreError::Security(format!(
                "sshd -t validation failed during lockdown: {}",
                check.stderr.trim()
            )));
        }

        let socket_active = runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "ssh.socket"],
                Duration::from_secs(5),
            )
            .map(|o| o.success())
            .unwrap_or(false);

        let restart_out = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15))?;
        let restart_ok = if restart_out.success() {
            true
        } else if socket_active {
            // On Debian 12/13 socket-activated hosts, sync systemd and restart `ssh.socket`.
            let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
            runner
                .run(
                    "systemctl",
                    &["restart", "ssh.socket"],
                    Duration::from_secs(15),
                )
                .map(|o| o.success())
                .unwrap_or(false)
        } else {
            false
        };

        if !restart_ok {
            restore_lockdown_files(wrote_dropin);
            let _ = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15));
            if socket_active {
                let _ = runner.run(
                    "systemctl",
                    &["restart", "ssh.socket"],
                    Duration::from_secs(15),
                );
            }
            return Err(CoreError::Security(format!(
                "failed to restart ssh during lockdown: {}",
                restart_out.stderr.trim()
            )));
        }

        let pw = runner.run("passwd", &["-l", "root"], Duration::from_secs(10))?;
        if !pw.success() {
            restore_lockdown_files(wrote_dropin);
            let _ = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15));
            if socket_active {
                let _ = runner.run(
                    "systemctl",
                    &["restart", "ssh.socket"],
                    Duration::from_secs(15),
                );
            }
            return Err(CoreError::Security(format!(
                "failed to lock root account: {}",
                pw.stderr.trim()
            )));
        }
    }

    let store = ConfigStore::new(ctx.effective_config_path());
    store.set("SSH_HARDENED", "true")?;
    Ok(())
}

/// Execute the full SSH zero-trust hardening workflow (`scripts/security.sh::check_ssh_security`).
pub fn check_ssh_security(
    ctx: &SecurityContext,
    prompter: &dyn SshPrompter,
    runner: &dyn CommandRunner,
) -> Result<SshHardenReport> {
    check_ssh_security_with_port(ctx, prompter, runner, None)
}

/// Same as [`check_ssh_security`] with an optional explicit port for deterministic testing.
pub fn check_ssh_security_with_port(
    ctx: &SecurityContext,
    prompter: &dyn SshPrompter,
    runner: &dyn CommandRunner,
    port_override: Option<u16>,
) -> Result<SshHardenReport> {
    if !ctx.non_interactive && prompter.requires_tty() && !io::stdin().is_terminal() {
        return Err(CoreError::ValidationFailed(
            "SSH hardening requires an interactive TTY (or explicit --non-interactive with --pubkey)"
                .to_string(),
        ));
    }

    if !ctx.non_interactive && !prompter.confirm_harden()? {
        return Ok(SshHardenReport {
            skipped: true,
            user: None,
            old_port: 0,
            new_port: 0,
            has_socket: false,
            locked_down: false,
            rolled_back: false,
        });
    }

    let target_user = detect_or_create_user(ctx, prompter, runner)?;
    setup_user_ssh_key(ctx, &target_user, prompter, runner)?;

    let old_port = fail2ban::detect_ssh_port(ctx, runner);
    let new_port = match port_override {
        Some(0) => {
            return Err(CoreError::ValidationFailed(
                "SSH port override must be in 1..=65535".to_string(),
            ));
        }
        Some(p) => p,
        None => random_high_port_excluding(old_port),
    };

    let has_socket = !ctx.no_sys
        && runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "ssh.socket"],
                Duration::from_secs(5),
            )
            .map(|o| o.success())
            .unwrap_or(false);

    let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
    let sshd_conf = ctx.path("/etc/ssh/sshd_config");
    let override_snapshot = fs::read(&override_path).ok();
    let sshd_snapshot = fs::read(&sshd_conf).ok();

    // Step 1: Open BOTH old_port and new_port in firewall BEFORE switching SSH port.
    let transition_ports = if old_port == new_port {
        vec![new_port]
    } else {
        vec![old_port, new_port]
    };
    firewall::update_ssh_firewall_ports(ctx, &transition_ports, runner)?;

    // Step 2: Switch SSH port; roll back firewall + SSH on any error.
    if let Err(err) = apply_ssh_port(ctx, new_port, has_socket, runner) {
        let _ = rollback_ssh_changes_with_snapshot(
            ctx,
            old_port,
            has_socket,
            new_port,
            override_snapshot.as_deref(),
            sshd_snapshot.as_deref(),
            runner,
        );
        return Err(err);
    }

    // Step 3: Verify login on new port; roll back on Err (I/O/EOF) OR unconfirmed (false).
    let confirmed = if ctx.non_interactive {
        true
    } else {
        match prompter.confirm_new_port_login(&target_user, new_port) {
            Ok(v) => v,
            Err(err) => {
                let _ = rollback_ssh_changes_with_snapshot(
                    ctx,
                    old_port,
                    has_socket,
                    new_port,
                    override_snapshot.as_deref(),
                    sshd_snapshot.as_deref(),
                    runner,
                );
                return Err(err);
            }
        }
    };

    if !confirmed {
        rollback_ssh_changes_with_snapshot(
            ctx,
            old_port,
            has_socket,
            new_port,
            override_snapshot.as_deref(),
            sshd_snapshot.as_deref(),
            runner,
        )?;
        return Ok(SshHardenReport {
            skipped: false,
            user: Some(target_user),
            old_port,
            new_port,
            has_socket,
            locked_down: false,
            rolled_back: true,
        });
    }

    // Step 4: Narrow firewall strictly to new_port after confirmation; roll back on Err.
    if let Err(err) = firewall::update_ssh_firewall(ctx, new_port, runner) {
        let _ = rollback_ssh_changes_with_snapshot(
            ctx,
            old_port,
            has_socket,
            new_port,
            override_snapshot.as_deref(),
            sshd_snapshot.as_deref(),
            runner,
        );
        return Err(err);
    }

    // Step 5: Lock down SSH authentication and root account; roll back firewall + SSH on Err.
    if let Err(err) = lockdown_ssh_system(ctx, runner) {
        let _ = rollback_ssh_changes_with_snapshot(
            ctx,
            old_port,
            has_socket,
            new_port,
            override_snapshot.as_deref(),
            sshd_snapshot.as_deref(),
            runner,
        );
        return Err(err);
    }
    Ok(SshHardenReport {
        skipped: false,
        user: Some(target_user),
        old_port,
        new_port,
        has_socket,
        locked_down: true,
        rolled_back: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};
    use std::cell::RefCell;
    use tempfile::TempDir;

    const VALID_ED25519_KEY_1: &str =
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl ops@test";
    const VALID_ED25519_KEY_2: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKk5qF9X4xB2l6V9tN8jR2wP7cY3mH1qL5vE8zA4sD9f alice@host";

    fn ok_out() -> CmdOutput {
        CmdOutput {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn err_out() -> CmdOutput {
        CmdOutput {
            code: 1,
            stdout: String::new(),
            stderr: "mock failure".to_string(),
            timed_out: false,
        }
    }

    struct ScriptedPrompter {
        harden: bool,
        use_existing: bool,
        usernames: RefCell<Vec<String>>,
        pubkeys: RefCell<Vec<String>>,
        confirm_login: bool,
        confirm_login_err: bool,
    }

    impl ScriptedPrompter {
        fn new(harden: bool, use_existing: bool, confirm_login: bool) -> Self {
            Self {
                harden,
                use_existing,
                usernames: RefCell::new(vec![]),
                pubkeys: RefCell::new(vec![]),
                confirm_login,
                confirm_login_err: false,
            }
        }
    }

    impl SshPrompter for ScriptedPrompter {
        fn confirm_harden(&self) -> Result<bool> {
            Ok(self.harden)
        }

        fn confirm_use_existing_user(&self, _username: &str) -> Result<bool> {
            Ok(self.use_existing)
        }

        fn prompt_new_username(&self) -> Result<String> {
            let mut q = self.usernames.borrow_mut();
            if q.is_empty() {
                Ok("admin".to_string())
            } else {
                Ok(q.remove(0))
            }
        }

        fn prompt_ssh_pubkey(&self) -> Result<String> {
            let mut q = self.pubkeys.borrow_mut();
            if q.is_empty() {
                Ok(String::new())
            } else {
                Ok(q.remove(0))
            }
        }

        fn confirm_new_port_login(&self, _user: &str, _new_port: u16) -> Result<bool> {
            if self.confirm_login_err {
                return Err(CoreError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "simulated EOF during login confirmation",
                )));
            }
            Ok(self.confirm_login)
        }
    }

    fn seed_nft_and_sshd(ctx: &SecurityContext) {
        let engine = ctx.template_engine();
        let mut vars = BTreeMap::new();
        vars.insert("SSH_PORT".to_string(), "22".to_string());
        vars.insert(
            "NFT_BASE_D".to_string(),
            ctx.effective_nft_base_d().to_string_lossy().into_owned(),
        );
        engine
            .render_to_file(
                "templates/security/nftables.conf",
                &ctx.effective_nft_main_conf(),
                &vars,
                Some(0o644),
            )
            .unwrap();
        let sshd = ctx.path("/etc/ssh/sshd_config");
        fs::create_dir_all(sshd.parent().unwrap()).unwrap();
        fs::write(
            &sshd,
            "#Port 22\nPermitRootLogin yes\nPasswordAuthentication yes\n",
        )
        .unwrap();
        fs::set_permissions(&sshd, fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn username_validation_and_random_high_port_bounds() {
        assert!(is_valid_username("alice"));
        assert!(is_valid_username("admin-01"));
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("Alice"));
        assert!(!is_valid_username("1admin"));
        assert!(!is_valid_username("bad_user"));

        assert!(is_valid_ed25519_pubkey(VALID_ED25519_KEY_1));
        assert!(!is_valid_ed25519_pubkey("ssh-ed25519 short"));
        assert!(!is_valid_ed25519_pubkey(
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCbad"
        ));
        assert!(!is_valid_ed25519_pubkey(&format!(
            "{VALID_ED25519_KEY_1}\ncommand=\"id\" {VALID_ED25519_KEY_2}"
        )));

        for _ in 0..32 {
            let p = random_high_port();
            assert!((40000..=65534).contains(&p), "port out of bounds: {p}");
        }
    }

    #[test]
    fn random_high_port_excluding_avoids_old_port() {
        for _ in 0..16 {
            let p = random_high_port_excluding(51234);
            assert!((40000..=65534).contains(&p));
            assert_ne!(p, 51234);
        }
    }

    #[test]
    fn check_ssh_security_skip_when_declined() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();
        let prompter = ScriptedPrompter::new(false, true, false);

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(45000)).unwrap();
        assert!(rep.skipped);
        assert!(!rep.locked_down);
    }

    #[test]
    fn ssh_harden_non_tty_without_explicit_flag_refuses_lockdown() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = true;
        ctx.allow_test_ephemeral_auth = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        seed_nft_and_sshd(&ctx);

        fs::create_dir_all(ctx.path("/etc")).unwrap();
        fs::write(ctx.path("/etc/passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("id", &["admin"], err_out());

        // Case 1: New user creation without explicit password refuses default "admin123" BEFORE useradd
        let prompter = NonInteractivePrompter {
            username: Some("admin".to_string()),
            password: None,
            ssh_pubkey: Some(VALID_ED25519_KEY_1.to_string()),
        };
        let err = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(51234)).unwrap_err();
        assert!(matches!(err, CoreError::ValidationFailed(_)));
        assert!(
            !runner.invocations().iter().any(|(p, _)| p == "useradd"),
            "useradd must not run when password validation fails"
        );
        assert!(!ctx.path("/opt/debopty/test_key").exists());

        // Case 2: Existing user with empty pubkey refuses ephemeral test_key generation
        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:alice\n").unwrap();
        let existing_keys = ctx.path("/home/alice/.ssh/authorized_keys");
        fs::create_dir_all(existing_keys.parent().unwrap()).unwrap();
        fs::write(&existing_keys, format!("{VALID_ED25519_KEY_2}\n")).unwrap();

        let empty_key_prompter = NonInteractivePrompter {
            username: None,
            password: None,
            ssh_pubkey: Some(String::new()),
        };
        let err2 = check_ssh_security_with_port(&ctx, &empty_key_prompter, &runner, Some(51234))
            .unwrap_err();
        assert!(matches!(err2, CoreError::ValidationFailed(_)));
        assert!(!ctx.path("/opt/debopty/test_key").exists());
        assert_eq!(
            fs::read_to_string(&existing_keys).unwrap(),
            format!("{VALID_ED25519_KEY_2}\n")
        );
        let sshd_txt = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(sshd_txt.contains("PermitRootLogin yes"));

        // Case 3: Password with newline injection rejected before useradd
        fs::write(ctx.path("/etc/passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        let injected_pass_prompter = NonInteractivePrompter {
            username: Some("opsuser".to_string()),
            password: Some("goodpass\nroot:hacked".to_string()),
            ssh_pubkey: Some(VALID_ED25519_KEY_1.to_string()),
        };
        let err3 =
            check_ssh_security_with_port(&ctx, &injected_pass_prompter, &runner, Some(51234))
                .unwrap_err();
        assert!(matches!(err3, CoreError::ValidationFailed(_)));
    }

    #[test]
    fn interactive_new_user_uses_run_interactive_for_passwd() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = false;
        fs::create_dir_all(ctx.path("/etc")).unwrap();
        fs::write(ctx.path("/etc/passwd"), "root:x:0:0:root:/root:/bin/bash\n").unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("id", &["newops"], err_out());
        runner.on_exact("id", &["-nG", "newops"], ok_out());
        runner.on_prog("useradd", ok_out());
        runner.on_prog("passwd", ok_out());
        runner.on_prog("usermod", ok_out());

        let prompter = ScriptedPrompter::new(true, false, true);
        prompter.usernames.borrow_mut().push("newops".to_string());

        let user = detect_or_create_user(&ctx, &prompter, &runner).unwrap();
        assert_eq!(user, "newops");
        assert_eq!(
            runner.interactive_invocations(),
            vec![("passwd".to_string(), vec!["newops".to_string()])]
        );
    }

    #[test]
    fn authorized_keys_appends_without_wiping_existing_and_rejects_multiline_key() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = true;
        fs::create_dir_all(ctx.path("/etc")).unwrap();
        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();

        let auth_keys = ctx.path("/home/alice/.ssh/authorized_keys");
        fs::create_dir_all(auth_keys.parent().unwrap()).unwrap();
        fs::write(&auth_keys, format!("{VALID_ED25519_KEY_2}\n")).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("chown", ok_out());

        // Multiline injection rejected without modifying authorized_keys
        let bad_prompter = NonInteractivePrompter {
            username: None,
            password: None,
            ssh_pubkey: Some(format!("{VALID_ED25519_KEY_1}\nextra-line")),
        };
        let err = setup_user_ssh_key(&ctx, "alice", &bad_prompter, &runner).unwrap_err();
        assert!(matches!(err, CoreError::ValidationFailed(_)));
        assert_eq!(
            fs::read_to_string(&auth_keys).unwrap(),
            format!("{VALID_ED25519_KEY_2}\n")
        );

        // Valid key appends without wiping existing key, uses `chown -R alice:`
        let ok_prompter = NonInteractivePrompter {
            username: None,
            password: None,
            ssh_pubkey: Some(VALID_ED25519_KEY_1.to_string()),
        };
        setup_user_ssh_key(&ctx, "alice", &ok_prompter, &runner).unwrap();
        let content = fs::read_to_string(&auth_keys).unwrap();
        assert!(content.contains(VALID_ED25519_KEY_2));
        assert!(content.contains(VALID_ED25519_KEY_1));
        assert_eq!(content.lines().count(), 2);
        assert_eq!(
            fs::metadata(&auth_keys).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(
            runner
                .invocations()
                .iter()
                .any(|(p, args)| p == "chown" && args.get(1).map(String::as_str) == Some("alice:"))
        );

        // Re-running with same key deduplicates (still 2 lines)
        setup_user_ssh_key(&ctx, "alice", &ok_prompter, &runner).unwrap();
        assert_eq!(fs::read_to_string(&auth_keys).unwrap().lines().count(), 2);

        // chown failure propagates error (never silently leaves root-owned authorized_keys)
        let fail_runner = MockCommandRunner::new();
        fail_runner.on_prog("chown", err_out());
        let chown_err = setup_user_ssh_key(&ctx, "alice", &ok_prompter, &fail_runner).unwrap_err();
        assert!(matches!(chown_err, CoreError::Security(_)));
    }

    #[test]
    fn check_ssh_security_full_lockdown_with_new_user_and_socket() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = true;
        ctx.f2b_backend_override = Some("systemd".to_string());
        seed_nft_and_sshd(&ctx);

        fs::create_dir_all(ctx.path("/etc")).unwrap();
        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nnobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n",
        )
        .unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("id", &["opsuser"], err_out());
        runner.on_exact("id", &["-nG", "opsuser"], ok_out());
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            ok_out(),
        );
        runner.on_prog("useradd", ok_out());
        runner.on_prog("chpasswd", ok_out());
        runner.on_prog("usermod", ok_out());
        runner.on_prog("chown", ok_out());
        runner.on_prog("sshd", ok_out());
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("nft", ok_out());
        runner.on_prog("passwd", ok_out());

        let prompter = NonInteractivePrompter {
            username: Some("opsuser".to_string()),
            password: Some("StrongPass!234".to_string()),
            ssh_pubkey: Some(VALID_ED25519_KEY_1.to_string()),
        };

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(51234)).unwrap();
        assert!(!rep.skipped);
        assert!(rep.locked_down);
        assert!(!rep.rolled_back);
        assert_eq!(rep.user.as_deref(), Some("opsuser"));
        assert_eq!(rep.new_port, 51234);
        assert!(rep.has_socket);

        // Verify chpasswd invoked via stdin with the explicit NonInteractivePrompter.password
        let stdin_calls = runner.stdin_invocations();
        assert_eq!(stdin_calls.len(), 1);
        assert_eq!(stdin_calls[0].0, "chpasswd");
        assert_eq!(stdin_calls[0].2, "opsuser:StrongPass!234\n");

        // Verify firewall opened BEFORE restarting ssh.socket, and sshd -t ran before lockdown restart
        let calls = runner.invocations();
        let first_nft_idx = calls.iter().position(|(p, _)| p == "nft").unwrap();
        let socket_restart_idx = calls
            .iter()
            .position(|(p, a)| p == "systemctl" && a == &["restart", "ssh.socket"])
            .unwrap();
        assert!(
            first_nft_idx < socket_restart_idx,
            "firewall must open new port before ssh.socket restarts"
        );
        let sshd_t_idx = calls
            .iter()
            .position(|(p, a)| p == "sshd" && a == &["-t"])
            .unwrap();
        let ssh_restart_idx = calls
            .iter()
            .position(|(p, a)| p == "systemctl" && a == &["restart", "ssh"])
            .unwrap();
        assert!(sshd_t_idx < ssh_restart_idx);

        // Verify authorized_keys mode 0600 and .ssh mode 0700
        let auth_keys = ctx.path("/home/opsuser/.ssh/authorized_keys");
        assert!(auth_keys.is_file());
        assert_eq!(
            fs::metadata(&auth_keys).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(ctx.path("/home/opsuser/.ssh"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        // Verify ssh.socket override rendered
        let override_txt =
            fs::read_to_string(ctx.path("/etc/systemd/system/ssh.socket.d/override.conf")).unwrap();
        assert!(override_txt.contains("ListenStream=51234"));

        // Verify sshd_config lockdown + mode 0644 preserved + ConfigStore SSH_HARDENED=true
        let sshd_path = ctx.path("/etc/ssh/sshd_config");
        let sshd_txt = fs::read_to_string(&sshd_path).unwrap();
        assert!(sshd_txt.contains("PermitRootLogin no"));
        assert!(sshd_txt.contains("PasswordAuthentication no"));
        assert!(sshd_txt.contains("PubkeyAuthentication yes"));
        assert_eq!(
            fs::metadata(&sshd_path).unwrap().permissions().mode() & 0o777,
            0o644
        );

        let nft_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt.contains(r#"tcp dport 51234 accept comment "SSH_Access_Port""#));
        assert_eq!(
            fs::metadata(ctx.effective_nft_main_conf())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );

        let store = ConfigStore::new(ctx.effective_config_path());
        assert_eq!(store.get("SSH_HARDENED").unwrap().as_deref(), Some("true"));
    }

    #[test]
    fn check_ssh_security_rolls_back_on_unconfirmed_login() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        seed_nft_and_sshd(&ctx);

        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:alice\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            err_out(),
        );
        runner.on_prog("sshd", ok_out());
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("chown", ok_out());
        runner.on_prog("nft", ok_out());

        let prompter = ScriptedPrompter::new(true, true, false);
        prompter.pubkeys.borrow_mut().extend([
            "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCbad".to_string(),
            VALID_ED25519_KEY_2.to_string(),
        ]);

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(49999)).unwrap();
        assert!(rep.rolled_back);
        assert!(!rep.locked_down);
        assert_eq!(rep.old_port, 22);
        assert_eq!(rep.new_port, 49999);

        // Verify sshd_config and nftables.conf rolled back to port 22
        let sshd_txt = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(!sshd_txt.contains("Port 49999"));
        assert!(sshd_txt.contains("PermitRootLogin yes"));

        let nft_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt.contains(r#"tcp dport 22 accept comment "SSH_Access_Port""#));
    }

    #[test]
    fn ssh_harden_rolls_back_when_prompt_or_sshd_t_fails() {
        // 1. Prompt I/O error triggers full rollback of both SSH and nftables
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        seed_nft_and_sshd(&ctx);

        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:alice\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            err_out(),
        );
        runner.on_prog("sshd", ok_out());
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("chown", ok_out());
        runner.on_prog("nft", ok_out());

        let mut prompter = ScriptedPrompter::new(true, true, false);
        prompter.confirm_login_err = true;
        prompter
            .pubkeys
            .borrow_mut()
            .push(VALID_ED25519_KEY_2.to_string());

        let err = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(48888)).unwrap_err();
        assert!(matches!(err, CoreError::Io(_)));
        let sshd_txt = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(!sshd_txt.contains("Port 48888"));
        let nft_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt.contains(r#"tcp dport 22 accept comment "SSH_Access_Port""#));

        // 2. sshd -t failure inside apply_ssh_port rolls back both sshd_config and firewall
        let runner2 = MockCommandRunner::new();
        runner2.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            err_out(),
        );
        runner2.on_exact("sshd", &["-t"], err_out());
        runner2.on_exact("sshd", &["-T"], ok_out());
        runner2.on_prog("systemctl", ok_out());
        runner2.on_prog("chown", ok_out());
        runner2.on_prog("nft", ok_out());

        let prompter2 = ScriptedPrompter::new(true, true, true);
        prompter2
            .pubkeys
            .borrow_mut()
            .push(VALID_ED25519_KEY_2.to_string());
        let err2 =
            check_ssh_security_with_port(&ctx, &prompter2, &runner2, Some(47777)).unwrap_err();
        assert!(matches!(err2, CoreError::Security(_)));
        let sshd_txt2 = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(!sshd_txt2.contains("Port 47777"));
        let nft_txt2 = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt2.contains(r#"tcp dport 22 accept comment "SSH_Access_Port""#));
    }

    #[test]
    fn ssh_socket_rollback_preserves_custom_old_port() {
        let tmp = TempDir::new().unwrap();
        let mut ctx = SecurityContext::with_root(tmp.path());
        ctx.non_interactive = false;
        ctx.f2b_backend_override = Some("systemd".to_string());
        seed_nft_and_sshd(&ctx);

        let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
        fs::create_dir_all(override_path.parent().unwrap()).unwrap();
        fs::write(
            &override_path,
            "[Socket]\nListenStream=\nListenStream=2222\n",
        )
        .unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("nft", ok_out());

        // Direct rollback_ssh_changes (even without snapshot) must restore ListenStream=2222
        fs::write(
            &override_path,
            "[Socket]\nListenStream=\nListenStream=55555\n",
        )
        .unwrap();
        rollback_ssh_changes(&ctx, 2222, true, 55555, &runner).unwrap();

        let restored = fs::read_to_string(&override_path).unwrap();
        assert!(
            restored.contains("ListenStream=2222"),
            "override.conf must preserve custom old_port 2222, got:\n{restored}"
        );
        let nft_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt.contains(r#"tcp dport 2222 accept comment "SSH_Access_Port""#));
    }

    #[test]
    fn existing_user_sudo_checked_and_usermod_failure_blocks_lockdown() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        fs::create_dir_all(ctx.path("/etc")).unwrap();
        fs::write(
            ctx.path("/etc/passwd"),
            "root:x:0:0:root:/root:/bin/bash\nalice:x:1000:1000:Alice:/home/alice:/bin/bash\n",
        )
        .unwrap();
        fs::write(ctx.path("/etc/group"), "sudo:x:27:\n").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact("id", &["-nG", "alice"], ok_out());
        runner.on_prog("usermod", err_out());

        let prompter = ScriptedPrompter::new(true, true, true);
        let err = detect_or_create_user(&ctx, &prompter, &runner).unwrap_err();
        assert!(matches!(err, CoreError::Security(_)));
    }

    #[test]
    fn lockdown_writes_dropin_and_inserts_directives_above_match_block() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        fs::create_dir_all(sshd_conf.parent().unwrap()).unwrap();
        fs::write(
            &sshd_conf,
            "Include /etc/ssh/sshd_config.d/*.conf\n\nMatch User backup\n    ForceCommand internal-sftp\n",
        )
        .unwrap();
        fs::set_permissions(&sshd_conf, fs::Permissions::from_mode(0o644)).unwrap();

        let runner = MockCommandRunner::new();
        runner.on_prog("sshd", ok_out());
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("passwd", ok_out());

        apply_ssh_port(&ctx, 51234, false, &runner).unwrap();
        lockdown_ssh_system(&ctx, &runner).unwrap();

        let dropin = ctx.path(HARDENING_DROPIN_PATH);
        assert!(dropin.is_file());
        let dropin_txt = fs::read_to_string(&dropin).unwrap();
        assert!(dropin_txt.contains("PermitRootLogin no"));
        assert!(dropin_txt.contains("PasswordAuthentication no"));
        assert!(dropin_txt.contains("PubkeyAuthentication yes"));
        assert_eq!(
            fs::metadata(&dropin).unwrap().permissions().mode() & 0o777,
            0o644
        );

        let sshd_txt = fs::read_to_string(&sshd_conf).unwrap();
        let match_pos = sshd_txt.find("Match User backup").unwrap();
        assert!(sshd_txt.find("Port 51234").unwrap() < match_pos);
        assert!(sshd_txt.find("PermitRootLogin no").unwrap() < match_pos);
        assert!(sshd_txt.find("PasswordAuthentication no").unwrap() < match_pos);
        assert!(sshd_txt.find("PubkeyAuthentication yes").unwrap() < match_pos);
        assert_eq!(
            fs::metadata(&sshd_conf).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn lockdown_selects_challenge_response_on_openssh_lt_8_7_and_kbd_interactive_on_ge_8_7() {
        assert_eq!(
            parse_openssh_version("OpenSSH_7.9p1 Debian-10+deb10u4, OpenSSL 1.1.1n"),
            Some((7, 9))
        );
        assert_eq!(
            parse_openssh_version("OpenSSH_8.4p1 Debian-5+deb11u3, OpenSSL 1.1.1w"),
            Some((8, 4))
        );
        assert_eq!(
            parse_openssh_version("OpenSSH_8.7p1, OpenSSL 1.1.1"),
            Some((8, 7))
        );
        assert_eq!(
            parse_openssh_version("OpenSSH_9.2p1 Debian-2+deb12u3, OpenSSL 3.0.15"),
            Some((9, 2))
        );

        // 1. Debian 11 (OpenSSH 8.4): `sshd -V` exits non-zero while printing version to stderr
        {
            let tmp = TempDir::new().unwrap();
            let ctx = SecurityContext::with_root(tmp.path());
            let sshd_conf = ctx.path("/etc/ssh/sshd_config");
            fs::create_dir_all(sshd_conf.parent().unwrap()).unwrap();
            fs::write(
                &sshd_conf,
                "Include /etc/ssh/sshd_config.d/*.conf\nPermitRootLogin yes\nPasswordAuthentication yes\nChallengeResponseAuthentication yes\n",
            )
            .unwrap();

            let runner = MockCommandRunner::new();
            runner.on_exact(
                "sshd",
                &["-V"],
                CmdOutput {
                    code: 1,
                    stdout: String::new(),
                    stderr: "unknown option -- V\nOpenSSH_8.4p1 Debian-5+deb11u3, OpenSSL 1.1.1w\n"
                        .to_string(),
                    timed_out: false,
                },
            );
            runner.on_exact("sshd", &["-t"], ok_out());
            runner.on_prog("systemctl", ok_out());
            runner.on_prog("passwd", ok_out());

            lockdown_ssh_system(&ctx, &runner).unwrap();

            let dropin_txt = fs::read_to_string(ctx.path(HARDENING_DROPIN_PATH)).unwrap();
            assert!(dropin_txt.contains("ChallengeResponseAuthentication no"));
            assert!(!dropin_txt.contains("KbdInteractiveAuthentication"));

            let sshd_txt = fs::read_to_string(&sshd_conf).unwrap();
            assert!(sshd_txt.contains("ChallengeResponseAuthentication no"));
            assert!(!sshd_txt.contains("KbdInteractiveAuthentication"));
        }

        // 2. Debian 12 (OpenSSH 9.2): emits KbdInteractiveAuthentication no
        {
            let tmp = TempDir::new().unwrap();
            let ctx = SecurityContext::with_root(tmp.path());
            let sshd_conf = ctx.path("/etc/ssh/sshd_config");
            fs::create_dir_all(sshd_conf.parent().unwrap()).unwrap();
            fs::write(
                &sshd_conf,
                "Include /etc/ssh/sshd_config.d/*.conf\nPermitRootLogin yes\nPasswordAuthentication yes\n",
            )
            .unwrap();

            let runner = MockCommandRunner::new();
            runner.on_exact(
                "sshd",
                &["-V"],
                CmdOutput {
                    code: 0,
                    stdout: String::new(),
                    stderr: "OpenSSH_9.2p1 Debian-2+deb12u3, OpenSSL 3.0.15\n".to_string(),
                    timed_out: false,
                },
            );
            runner.on_exact("sshd", &["-t"], ok_out());
            runner.on_prog("systemctl", ok_out());
            runner.on_prog("passwd", ok_out());

            lockdown_ssh_system(&ctx, &runner).unwrap();

            let dropin_txt = fs::read_to_string(ctx.path(HARDENING_DROPIN_PATH)).unwrap();
            assert!(dropin_txt.contains("KbdInteractiveAuthentication no"));
            assert!(!dropin_txt.contains("ChallengeResponseAuthentication"));

            let sshd_txt = fs::read_to_string(&sshd_conf).unwrap();
            assert!(sshd_txt.contains("KbdInteractiveAuthentication no"));
        }

        // 3. Offline/no_sys fallback uses /etc/os-release (Debian 10 -> ChallengeResponseAuthentication)
        {
            let tmp = TempDir::new().unwrap();
            fs::create_dir_all(tmp.path().join("etc")).unwrap();
            fs::write(
                tmp.path().join("etc/os-release"),
                "ID=debian\nVERSION_ID=\"10\"\n",
            )
            .unwrap();
            let runner = MockCommandRunner::new();
            assert_eq!(
                preferred_kbd_interactive_directive(tmp.path(), true, &runner),
                "ChallengeResponseAuthentication"
            );
        }
    }
}
