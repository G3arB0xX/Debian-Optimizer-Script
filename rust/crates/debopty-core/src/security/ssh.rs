use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::ConfigStore;
use crate::error::{CoreError, Result};
use crate::security::f2b_sync;
use crate::security::fail2ban;
use crate::security::firewall;
use crate::security::{ensure_trailing_newline, write_file_atomic};
use crate::sys::{self, CommandRunner};

pub use crate::security::SecurityContext;

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
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StdioPrompter;

impl StdioPrompter {
    fn read_line(prompt: &str) -> Result<String> {
        print!("{prompt}");
        io::stdout().flush()?;
        let mut line = String::new();
        let stdin = io::stdin();
        let _ = stdin.lock().read_line(&mut line)?;
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
            let pass = std::env::var("TEST_USER_PASSWORD")
                .ok()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "admin123".to_string());
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
            let pw = runner.run("passwd", &[&username], Duration::from_secs(120))?;
            if !pw.success() {
                return Err(CoreError::Security(format!(
                    "passwd failed for {username}: {}",
                    pw.stderr.trim()
                )));
            }
        }

        let group_content = fs::read_to_string(ctx.path("/etc/group")).unwrap_or_default();
        if group_content.lines().any(|l| l.starts_with("sudo:")) {
            let _ = runner.run(
                "usermod",
                &["-aG", "sudo", &username],
                Duration::from_secs(10),
            );
        } else if group_content.lines().any(|l| l.starts_with("wheel:")) {
            let _ = runner.run(
                "usermod",
                &["-aG", "wheel", &username],
                Duration::from_secs(10),
            );
        }
    }

    Ok(username)
}

/// Inject an Ed25519 SSH public key into the target user's `authorized_keys`.
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

    let user_home = ctx.path(&raw_home.to_string_lossy());
    let ssh_dir = user_home.join(".ssh");
    fs::create_dir_all(&ssh_dir)?;
    fs::set_permissions(&ssh_dir, fs::Permissions::from_mode(0o700))?;

    let mut pub_key = prompter.prompt_ssh_pubkey()?.trim().to_string();

    if ctx.non_interactive {
        if pub_key.is_empty() {
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
    } else {
        let mut attempts = 0;
        while pub_key.is_empty() || !pub_key.contains("ssh-ed25519") {
            attempts += 1;
            if attempts >= 5 {
                return Err(CoreError::ValidationFailed(
                    "valid ssh-ed25519 public key required".to_string(),
                ));
            }
            pub_key = prompter.prompt_ssh_pubkey()?.trim().to_string();
        }
    }

    if pub_key.is_empty() {
        return Err(CoreError::ValidationFailed(
            "SSH public key cannot be empty".to_string(),
        ));
    }

    let auth_keys = ssh_dir.join("authorized_keys");
    let content = ensure_trailing_newline(pub_key);
    write_file_atomic(&auth_keys, content.as_bytes(), Some(0o600))?;

    if !ctx.no_sys {
        let owner = format!("{user}:{user}");
        let ssh_dir_str = ssh_dir.to_string_lossy();
        let _ = runner.run(
            "chown",
            &["-R", &owner, &ssh_dir_str],
            Duration::from_secs(10),
        );
    }

    Ok(auth_keys)
}

/// Apply new SSH listening port via `ssh.socket` override or `/etc/ssh/sshd_config`.
pub fn apply_ssh_port(
    ctx: &SecurityContext,
    port: u16,
    has_socket: bool,
    runner: &dyn CommandRunner,
) -> Result<()> {
    if has_socket {
        let engine = ctx.template_engine();
        let mut vars = BTreeMap::new();
        vars.insert("PORT".to_string(), port.to_string());
        let rendered = ensure_trailing_newline(
            engine.render_to_string("templates/security/ssh.socket.override.conf", &vars)?,
        );
        write_file_atomic(
            &ctx.path("/etc/systemd/system/ssh.socket.d/override.conf"),
            rendered.as_bytes(),
            Some(0o644),
        )?;

        if !ctx.no_sys {
            let _ = runner.run("systemctl", &["daemon-reload"], Duration::from_secs(15));
            let out = runner.run(
                "systemctl",
                &["restart", "ssh.socket"],
                Duration::from_secs(15),
            )?;
            if !out.success() {
                return Err(CoreError::Security(format!(
                    "failed to restart ssh.socket: {}",
                    out.stderr.trim()
                )));
            }
        }
    } else {
        let sshd_conf = ctx.path("/etc/ssh/sshd_config");
        sys::set_conf_value(&sshd_conf, "Port", &port.to_string(), " ")?;

        if !ctx.no_sys {
            let out = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15))?;
            if !out.success() {
                return Err(CoreError::Security(format!(
                    "failed to restart ssh: {}",
                    out.stderr.trim()
                )));
            }
        }
    }

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
    if has_socket {
        let override_path = ctx.path("/etc/systemd/system/ssh.socket.d/override.conf");
        if override_path.exists() {
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
        if sshd_conf.is_file() {
            let content = fs::read_to_string(&sshd_conf)?;
            let target = format!("Port {new_port}");
            let replacement = format!("Port {old_port}");
            let mut out_lines: Vec<String> = Vec::new();
            for line in content.lines() {
                if line.trim_start() == target {
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

    firewall::update_ssh_firewall(ctx, old_port, runner)?;

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

    if !ctx.no_sys {
        let main_conf = ctx.effective_nft_main_conf();
        if main_conf.is_file() {
            let conf_str = main_conf.to_string_lossy();
            let _ = runner.run("nft", &["-f", &conf_str], Duration::from_secs(15));
        }
    }

    let engine = ctx.template_engine();
    f2b_sync::run_f2b_sync(&ctx.f2b_sync_options(true), &engine, runner)?;
    Ok(())
}

/// Disable root login & password authentication, lock `root`, and persist `SSH_HARDENED="true"`.
pub fn lockdown_ssh_system(ctx: &SecurityContext, runner: &dyn CommandRunner) -> Result<()> {
    let sshd_conf = ctx.path("/etc/ssh/sshd_config");
    sys::set_conf_value(&sshd_conf, "PermitRootLogin", "no", " ")?;
    sys::set_conf_value(&sshd_conf, "PasswordAuthentication", "no", " ")?;
    sys::set_conf_value(&sshd_conf, "PubkeyAuthentication", "yes", " ")?;

    if !ctx.no_sys {
        let out = runner.run("systemctl", &["restart", "ssh"], Duration::from_secs(15))?;
        if !out.success() {
            return Err(CoreError::Security(format!(
                "failed to restart ssh during lockdown: {}",
                out.stderr.trim()
            )));
        }
        let pw = runner.run("passwd", &["-l", "root"], Duration::from_secs(10))?;
        if !pw.success() {
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
    let new_port = port_override.unwrap_or_else(random_high_port);

    let has_socket = !ctx.no_sys
        && runner
            .run(
                "systemctl",
                &["is-active", "--quiet", "ssh.socket"],
                Duration::from_secs(5),
            )
            .map(|o| o.success())
            .unwrap_or(false);

    apply_ssh_port(ctx, new_port, has_socket, runner)?;
    firewall::update_ssh_firewall(ctx, new_port, runner)?;

    let confirmed = if ctx.non_interactive {
        true
    } else {
        prompter.confirm_new_port_login(&target_user, new_port)?
    };

    if !confirmed {
        rollback_ssh_changes(ctx, old_port, has_socket, new_port, runner)?;
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

    lockdown_ssh_system(ctx, runner)?;
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
            stderr: String::new(),
            timed_out: false,
        }
    }

    struct ScriptedPrompter {
        harden: bool,
        use_existing: bool,
        usernames: RefCell<Vec<String>>,
        pubkeys: RefCell<Vec<String>>,
        confirm_login: bool,
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
    }

    #[test]
    fn username_validation_and_random_high_port_bounds() {
        assert!(is_valid_username("alice"));
        assert!(is_valid_username("admin-01"));
        assert!(!is_valid_username(""));
        assert!(!is_valid_username("Alice"));
        assert!(!is_valid_username("1admin"));
        assert!(!is_valid_username("bad_user"));

        for _ in 0..32 {
            let p = random_high_port();
            assert!((40000..=65534).contains(&p), "port out of bounds: {p}");
        }
    }

    #[test]
    fn check_ssh_security_skip_when_declined() {
        let tmp = TempDir::new().unwrap();
        let ctx = SecurityContext::with_root(tmp.path());
        let runner = MockCommandRunner::new();
        let prompter = ScriptedPrompter {
            harden: false,
            use_existing: true,
            usernames: RefCell::new(vec![]),
            pubkeys: RefCell::new(vec![]),
            confirm_login: false,
        };

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(45000)).unwrap();
        assert!(rep.skipped);
        assert!(!rep.locked_down);
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
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            ok_out(),
        );
        runner.on_prog("useradd", ok_out());
        runner.on_prog("chpasswd", ok_out());
        runner.on_prog("usermod", ok_out());
        runner.on_prog("chown", ok_out());
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("nft", ok_out());
        runner.on_prog("passwd", ok_out());

        let prompter = NonInteractivePrompter {
            username: Some("opsuser".to_string()),
            password: None,
            ssh_pubkey: Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAITestKey ops@test".to_string()),
        };

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(51234)).unwrap();
        assert!(!rep.skipped);
        assert!(rep.locked_down);
        assert!(!rep.rolled_back);
        assert_eq!(rep.user.as_deref(), Some("opsuser"));
        assert_eq!(rep.new_port, 51234);
        assert!(rep.has_socket);

        // Verify chpasswd invoked via stdin (no shell pipeline)
        let stdin_calls = runner.stdin_invocations();
        assert_eq!(stdin_calls.len(), 1);
        assert_eq!(stdin_calls[0].0, "chpasswd");
        assert!(stdin_calls[0].2.starts_with("opsuser:"));

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

        // Verify sshd_config lockdown + ConfigStore SSH_HARDENED=true
        let sshd_txt = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(sshd_txt.contains("PermitRootLogin no"));
        assert!(sshd_txt.contains("PasswordAuthentication no"));
        assert!(sshd_txt.contains("PubkeyAuthentication yes"));

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

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "systemctl",
            &["is-active", "--quiet", "ssh.socket"],
            err_out(),
        );
        runner.on_prog("systemctl", ok_out());
        runner.on_prog("chown", ok_out());
        runner.on_prog("nft", ok_out());

        let prompter = ScriptedPrompter {
            harden: true,
            use_existing: true,
            usernames: RefCell::new(vec![]),
            // First rejects non-ed25519 key, second provides valid ed25519 key
            pubkeys: RefCell::new(vec![
                "ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCbad".to_string(),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAliceKey alice@host".to_string(),
            ]),
            confirm_login: false,
        };

        let rep = check_ssh_security_with_port(&ctx, &prompter, &runner, Some(49999)).unwrap();
        assert!(rep.rolled_back);
        assert!(!rep.locked_down);
        assert_eq!(rep.old_port, 22);
        assert_eq!(rep.new_port, 49999);

        // Verify sshd_config and nftables.conf rolled back to port 22
        let sshd_txt = fs::read_to_string(ctx.path("/etc/ssh/sshd_config")).unwrap();
        assert!(sshd_txt.contains("Port 22"));
        assert!(!sshd_txt.contains("Port 49999"));
        assert!(sshd_txt.contains("PermitRootLogin yes"));

        let nft_txt = fs::read_to_string(ctx.effective_nft_main_conf()).unwrap();
        assert!(nft_txt.contains(r#"tcp dport 22 accept comment "SSH_Access_Port""#));
    }
}
