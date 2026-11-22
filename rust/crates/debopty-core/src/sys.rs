use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use tempfile::NamedTempFile;
use wait_timeout::ChildExt;

use crate::error::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        !self.timed_out && self.code == 0
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput>;

    fn run_with_env(
        &self,
        prog: &str,
        args: &[&str],
        env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<CmdOutput> {
        let _ = env;
        self.run(prog, args, timeout)
    }

    fn run_with_stdin(
        &self,
        prog: &str,
        args: &[&str],
        stdin_data: &str,
        timeout: Duration,
    ) -> Result<CmdOutput> {
        let _ = stdin_data;
        self.run(prog, args, timeout)
    }

    /// Run an interactive command inheriting stdin/stdout/stderr (e.g. `passwd`).
    /// Returns the child exit code (`-1` if terminated by signal).
    fn run_interactive(&self, prog: &str, args: &[&str]) -> Result<i32> {
        let out = self.run(prog, args, Duration::from_secs(300))?;
        Ok(out.code)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealCommandRunner;

impl RealCommandRunner {
    fn spawn_and_wait(
        prog: &str,
        args: &[&str],
        env: &[(&str, &str)],
        stdin_data: Option<&str>,
        timeout: Duration,
    ) -> Result<CmdOutput> {
        let mut cmd = Command::new(prog);
        cmd.args(args)
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if stdin_data.is_some() {
            cmd.stdin(Stdio::piped());
        } else {
            cmd.stdin(Stdio::null());
        }

        // SAFETY: `setpgid(0, 0)` is POSIX async-signal-safe and isolates the child
        // process group so timeout termination can terminate grandchildren holding pipes.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let mut child = cmd.spawn().map_err(|e| CoreError::CommandFailed {
            prog: prog.to_string(),
            message: e.to_string(),
        })?;

        let stdin_rx = if let Some(data) = stdin_data {
            let owned = data.as_bytes().to_vec();
            let mut stdin_pipe = child.stdin.take();
            let (tx, rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                if let Some(ref mut pipe) = stdin_pipe {
                    let _ = pipe.write_all(&owned);
                    let _ = pipe.flush();
                }
                let _ = tx.send(());
            });
            Some((handle, rx))
        } else {
            None
        };

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let stdout_buf = Arc::new(Mutex::new(Vec::new()));
        let stdout_buf_t = Arc::clone(&stdout_buf);
        let (stdout_tx, stdout_rx) = mpsc::channel();
        let stdout_handle = thread::spawn(move || {
            if let Some(ref mut pipe) = stdout_pipe {
                let mut chunk = [0u8; 4096];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut b) = stdout_buf_t.lock() {
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            }
            let _ = stdout_tx.send(());
        });

        let stderr_buf = Arc::new(Mutex::new(Vec::new()));
        let stderr_buf_t = Arc::clone(&stderr_buf);
        let (stderr_tx, stderr_rx) = mpsc::channel();
        let stderr_handle = thread::spawn(move || {
            if let Some(ref mut pipe) = stderr_pipe {
                let mut chunk = [0u8; 4096];
                loop {
                    match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut b) = stderr_buf_t.lock() {
                                b.extend_from_slice(&chunk[..n]);
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
            }
            let _ = stderr_tx.send(());
        });

        let (code, timed_out) = match child.wait_timeout(timeout)? {
            Some(status) => (status.code().unwrap_or(-1), false),
            None => {
                let pid = child.id() as libc::pid_t;
                if pid > 0 {
                    // SAFETY: `killpg` targets the isolated process group created via `setpgid(0, 0)`.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                (-1, true)
            }
        };

        // Bounded drain on both timeout AND normal exit: if a child spawned a background daemon
        // that inherited stdout/stderr/stdin pipes, do not hang forever waiting on thread joins.
        let drain_budget = if timed_out {
            Duration::from_millis(200)
        } else {
            Duration::from_millis(500)
        };
        let deadline = Instant::now() + drain_budget;

        if let Some((h, rx)) = stdin_rx {
            let rem = deadline.saturating_duration_since(Instant::now());
            if rx.recv_timeout(rem).is_ok() {
                let _ = h.join();
            } else {
                drop(h);
            }
        }

        let rem_out = deadline.saturating_duration_since(Instant::now());
        if stdout_rx.recv_timeout(rem_out).is_ok() {
            let _ = stdout_handle.join();
        } else {
            drop(stdout_handle);
        }

        let rem_err = deadline.saturating_duration_since(Instant::now());
        if stderr_rx.recv_timeout(rem_err).is_ok() {
            let _ = stderr_handle.join();
        } else {
            drop(stderr_handle);
        }

        let stdout = String::from_utf8_lossy(&stdout_buf.lock().unwrap_or_else(|e| e.into_inner()))
            .into_owned();
        let stderr = String::from_utf8_lossy(&stderr_buf.lock().unwrap_or_else(|e| e.into_inner()))
            .into_owned();

        Ok(CmdOutput {
            code,
            stdout,
            stderr,
            timed_out,
        })
    }
}

impl CommandRunner for RealCommandRunner {
    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
        Self::spawn_and_wait(prog, args, &[], None, timeout)
    }

    fn run_with_env(
        &self,
        prog: &str,
        args: &[&str],
        env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<CmdOutput> {
        Self::spawn_and_wait(prog, args, env, None, timeout)
    }

    fn run_with_stdin(
        &self,
        prog: &str,
        args: &[&str],
        stdin_data: &str,
        timeout: Duration,
    ) -> Result<CmdOutput> {
        Self::spawn_and_wait(prog, args, &[], Some(stdin_data), timeout)
    }

    fn run_interactive(&self, prog: &str, args: &[&str]) -> Result<i32> {
        let status = Command::new(prog)
            .args(args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| CoreError::CommandFailed {
                prog: prog.to_string(),
                message: e.to_string(),
            })?;
        Ok(status.code().unwrap_or(-1))
    }
}

type EnvInvocation = (String, Vec<String>, Vec<(String, String)>);
type StdinInvocation = (String, Vec<String>, String);

#[derive(Debug, Default)]
pub struct MockCommandRunner {
    responses: Mutex<HashMap<(String, Vec<String>), CmdOutput>>,
    prog_defaults: Mutex<HashMap<String, CmdOutput>>,
    invocations: Mutex<Vec<(String, Vec<String>)>>,
    env_invocations: Mutex<Vec<EnvInvocation>>,
    stdin_invocations: Mutex<Vec<StdinInvocation>>,
    interactive_invocations: Mutex<Vec<(String, Vec<String>)>>,
}

impl MockCommandRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_exact(&self, prog: &str, args: &[&str], output: CmdOutput) {
        let key = (
            prog.to_string(),
            args.iter().map(|s| (*s).to_string()).collect(),
        );
        self.responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, output);
    }

    pub fn on_prog(&self, prog: &str, output: CmdOutput) {
        self.prog_defaults
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(prog.to_string(), output);
    }

    pub fn invocations(&self) -> Vec<(String, Vec<String>)> {
        self.invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn env_invocations(&self) -> Vec<EnvInvocation> {
        self.env_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn stdin_invocations(&self) -> Vec<StdinInvocation> {
        self.stdin_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn interactive_invocations(&self) -> Vec<(String, Vec<String>)> {
        self.interactive_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl CommandRunner for MockCommandRunner {
    fn run(&self, prog: &str, args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((prog.to_string(), arg_vec.clone()));

        if let Some(out) = self
            .responses
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(prog.to_string(), arg_vec))
        {
            return Ok(out.clone());
        }
        if let Some(out) = self
            .prog_defaults
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(prog)
        {
            return Ok(out.clone());
        }
        Err(CoreError::CommandFailed {
            prog: prog.to_string(),
            message: "command not mocked".to_string(),
        })
    }

    fn run_with_env(
        &self,
        prog: &str,
        args: &[&str],
        env: &[(&str, &str)],
        timeout: Duration,
    ) -> Result<CmdOutput> {
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        let env_vec: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        self.env_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((prog.to_string(), arg_vec, env_vec));
        self.run(prog, args, timeout)
    }

    fn run_with_stdin(
        &self,
        prog: &str,
        args: &[&str],
        stdin_data: &str,
        timeout: Duration,
    ) -> Result<CmdOutput> {
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.stdin_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((prog.to_string(), arg_vec, stdin_data.to_string()));
        self.run(prog, args, timeout)
    }

    fn run_interactive(&self, prog: &str, args: &[&str]) -> Result<i32> {
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.interactive_invocations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((prog.to_string(), arg_vec));
        let out = self.run(prog, args, Duration::from_secs(300))?;
        Ok(out.code)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
    pub gid: u32,
    pub home: PathBuf,
    pub shell: PathBuf,
}

/// Parse `/etc/passwd` content into structured entries.
pub fn parse_passwd(text: &str) -> Vec<PasswdEntry> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = trimmed.split(':').collect();
        if fields.len() < 7 {
            continue;
        }
        let Ok(uid) = fields[2].parse::<u32>() else {
            continue;
        };
        let Ok(gid) = fields[3].parse::<u32>() else {
            continue;
        };
        entries.push(PasswdEntry {
            name: fields[0].to_string(),
            uid,
            gid,
            home: PathBuf::from(fields[5]),
            shell: PathBuf::from(fields[6]),
        });
    }
    entries
}

/// Returns the first normal user (`uid >= 1000 && name != "nobody"`).
pub fn normal_user(entries: &[PasswdEntry]) -> Option<String> {
    entries
        .iter()
        .find(|e| e.uid >= 1000 && e.name != "nobody")
        .map(|e| e.name.clone())
}

/// Returns the Source-Of-Truth user (first normal user, or `"root"`).
pub fn sot_user(entries: &[PasswdEntry]) -> String {
    normal_user(entries).unwrap_or_else(|| "root".to_string())
}

/// Returns `"root"` plus all real interactive users (`uid >= 1000`, not `nobody`, existing home dir, non-nologin/false shell).
pub fn all_real_users(entries: &[PasswdEntry]) -> Vec<String> {
    all_real_users_with_dir_check(entries, |p| p.is_dir())
}

pub fn all_real_users_with_dir_check<F>(entries: &[PasswdEntry], dir_exists: F) -> Vec<String>
where
    F: Fn(&Path) -> bool,
{
    let mut users = vec!["root".to_string()];
    for entry in entries {
        if entry.uid >= 1000 && entry.name != "nobody" {
            let shell = entry.shell.to_string_lossy();
            if dir_exists(&entry.home) && !shell.ends_with("/nologin") && !shell.ends_with("/false")
            {
                users.push(entry.name.clone());
            }
        }
    }
    users
}

/// Returns the original user who invoked `debopty` via `sudo`, or `"root"`.
pub fn initial_user(sudo_user_env: Option<&str>) -> String {
    match sudo_user_env {
        Some(u) if !u.is_empty() && u != "root" => u.to_string(),
        _ => "root".to_string(),
    }
}

/// Resolve runtime `HOME` safely without shell `eval`.
pub fn resolve_runtime_home(
    euid: u32,
    current_home: Option<&str>,
    entries: &[PasswdEntry],
) -> PathBuf {
    if let Some(h) = current_home.filter(|s| !s.is_empty()) {
        return PathBuf::from(h);
    }
    if euid == 0 {
        return PathBuf::from("/root");
    }
    if let Some(entry) = entries.iter().find(|e| e.uid == euid)
        && !entry.home.as_os_str().is_empty()
    {
        return entry.home.clone();
    }
    PathBuf::from("/tmp")
}

/// Detect Debian major version from `/etc/debian_version` (e.g. `"12.5"` -> `Some(12)`, `"trixie/sid"` -> `Some(13)`).
pub fn detect_debian_major(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    if let Some(first) = trimmed.split('.').next()
        && let Ok(major) = first.parse::<u32>()
    {
        return Some(major);
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.contains("forky") {
        Some(14)
    } else if lower.contains("trixie") {
        Some(13)
    } else if lower.contains("bookworm") {
        Some(12)
    } else if lower.contains("bullseye") {
        Some(11)
    } else if lower.contains("buster") {
        Some(10)
    } else if lower.contains("sid") || lower.contains("testing") || lower.contains("unstable") {
        Some(13)
    } else {
        None
    }
}

pub fn current_euid() -> u32 {
    // SAFETY: `geteuid` is always safe to call on Unix/Linux.
    unsafe { libc::geteuid() }
}

pub fn current_egid() -> u32 {
    // SAFETY: `getegid` is always safe to call on Unix/Linux.
    unsafe { libc::getegid() }
}

fn apply_owner_fd(fd: RawFd, uid: u32, gid: u32) -> Result<()> {
    // SAFETY: `fd` is a valid open file descriptor belonging to the live `NamedTempFile`.
    let rc = unsafe { libc::fchown(fd, uid as libc::uid_t, gid as libc::gid_t) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if current_euid() != 0 && err.raw_os_error() == Some(libc::EPERM) {
            return Ok(());
        }
        return Err(CoreError::Io(err));
    }
    Ok(())
}

/// Change ownership of `path` to `(uid, gid)`.
///
/// Strictly enforces errors when running as root (`euid == 0`); tolerates `EPERM` when running
/// unprivileged (e.g. non-root host unit tests simulating `/etc/passwd` UIDs).
pub fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<()> {
    if let Err(err) = chown(path, Some(uid), Some(gid)) {
        if current_euid() != 0 && err.raw_os_error() == Some(libc::EPERM) {
            return Ok(());
        }
        return Err(CoreError::Io(err));
    }
    Ok(())
}

/// Resolve the effective permission mode for an atomic write to `path`.
///
/// Uses `mode` if explicitly provided, otherwise copies the existing target file's
/// permission bits (`mode & 0o7777`), or defaults to `0o644` when creating a new file.
pub fn resolve_atomic_write_mode(path: &Path, mode: Option<u32>) -> u32 {
    if let Some(m) = mode {
        return m;
    }
    fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0o644)
}

/// Apply resolved ownership and permissions (`Some(mode)` or preserve existing target mode,
/// defaulting to `0o644`) and atomically persist `tmp` onto `target`.
pub fn persist_preserving_mode_and_owner(
    tmp: NamedTempFile,
    target: &Path,
    mode: Option<u32>,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    let eff_mode = resolve_atomic_write_mode(target, mode);
    let eff_owner = owner.or_else(|| {
        if current_euid() == 0 {
            fs::metadata(target).ok().map(|m| (m.uid(), m.gid()))
        } else {
            None
        }
    });
    if let Some((uid, gid)) = eff_owner {
        apply_owner_fd(tmp.as_file().as_raw_fd(), uid, gid)?;
    }
    fs::set_permissions(tmp.path(), fs::Permissions::from_mode(eff_mode))?;
    tmp.persist(target).map_err(|e| CoreError::Io(e.error))?;
    Ok(())
}

/// Apply resolved permissions (`Some(mode)` or preserve existing target mode, defaulting to `0o644`)
/// and atomically persist `tmp` onto `target`.
pub fn persist_preserving_mode(tmp: NamedTempFile, target: &Path, mode: Option<u32>) -> Result<()> {
    persist_preserving_mode_and_owner(tmp, target, mode, None)
}

/// Atomically write `content` to `path`, creating parent directories, applying `owner` via `fchown`
/// before rename (preserving existing ownership when running as root and `owner` is `None`),
/// and preserving/applying permissions (defaulting to `0o644` when `mode` is `None`).
pub fn write_file_atomic_with_owner(
    path: &Path,
    content: &[u8],
    mode: Option<u32>,
    owner: Option<(u32, u32)>,
) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(content)?;
    tmp.flush()?;
    persist_preserving_mode_and_owner(tmp, path, mode, owner)
}

/// Atomically write `content` to `path`, creating parent directories and preserving permissions
/// (defaulting to `0o644` when `mode` is `None` and `path` does not yet exist).
pub fn write_file_atomic(path: &Path, content: &[u8], mode: Option<u32>) -> Result<()> {
    write_file_atomic_with_owner(path, content, mode, None)
}

/// Atomically update or append a key-value (or flag) line in a config file.
///
/// Matches optional leading whitespace / `#` and whitespace around `=` or whitespace separators,
/// deduplicating multiple matches in place or appending a new line if absent.
/// Preserves existing file permissions (defaulting to `0o644`).
pub fn set_conf_value(path: &Path, key: &str, value: &str, sep: &str) -> Result<()> {
    if key.is_empty()
        || key.contains(['\r', '\n', '\0'])
        || value.contains(['\r', '\n', '\0'])
        || sep.contains(['\r', '\n', '\0'])
    {
        return Err(CoreError::InvalidConfig(format!(
            "invalid control characters in set_conf_value for key '{key}'"
        )));
    }

    let existing = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let (pattern, replacement) = if value.is_empty() && sep.is_empty() {
        (
            format!(r"^\s*#?\s*{}\s*$", regex::escape(key)),
            key.to_string(),
        )
    } else if sep.trim() == "=" {
        (
            format!(r"^\s*#?\s*{}\s*=", regex::escape(key)),
            format!("{key}{sep}{value}"),
        )
    } else if !sep.is_empty() && sep.chars().all(char::is_whitespace) {
        (
            format!(r"^\s*#?\s*{}\s+", regex::escape(key)),
            format!("{key}{sep}{value}"),
        )
    } else {
        (
            format!(r"^\s*#?\s*{}{}", regex::escape(key), regex::escape(sep)),
            format!("{key}{sep}{value}"),
        )
    };

    let re = Regex::new(&pattern)
        .map_err(|e| CoreError::InvalidConfig(format!("invalid conf pattern: {e}")))?;

    let mut updated = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if re.is_match(line) {
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

    let mut rendered = out_lines.join("\n");
    rendered.push('\n');

    write_file_atomic(path, rendered.as_bytes(), None)
}

/// Return `true` iff `pkg` is fully installed (`dpkg-query -W -f=${Status}` == `"install ok installed"`).
pub fn is_dpkg_installed(pkg: &str, runner: &dyn CommandRunner) -> bool {
    runner
        .run(
            "dpkg-query",
            &["-W", "-f=${Status}", pkg],
            Duration::from_secs(15),
        )
        .map(|o| o.success() && o.stdout.trim() == "install ok installed")
        .unwrap_or(false)
}

/// Install only missing APT packages non-interactively; returns the list of installed packages.
pub fn safe_apt_install(pkgs: &[&str], runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let mut missing: Vec<&str> = Vec::new();
    for &pkg in pkgs {
        if !is_dpkg_installed(pkg, runner) {
            missing.push(pkg);
        }
    }

    if missing.is_empty() {
        return Ok(Vec::new());
    }

    let env = &[("DEBIAN_FRONTEND", "noninteractive")];
    let _ = runner.run_with_env("apt-get", &["update", "-yq"], env, Duration::from_secs(120));

    let mut install_args: Vec<&str> = Vec::with_capacity(2 + missing.len());
    install_args.push("install");
    install_args.push("-yq");
    install_args.extend_from_slice(&missing);

    let out = runner.run_with_env("apt-get", &install_args, env, Duration::from_secs(300))?;
    if !out.success() {
        return Err(CoreError::CommandFailed {
            prog: "apt-get".to_string(),
            message: format!(
                "apt-get install failed (code {}): {}",
                out.code,
                out.stderr.trim()
            ),
        });
    }

    Ok(missing.into_iter().map(String::from).collect())
}

/// Parse `MemTotal` in MiB from `/proc/meminfo` (or a fixture file).
pub fn read_mem_total_mb(meminfo_path: &Path) -> Option<u64> {
    let text = fs::read_to_string(meminfo_path).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb_str = rest.split_whitespace().next()?;
            let kb = kb_str.parse::<u64>().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

/// Map `uname -m` / `std::env::consts::ARCH` to Debian kernel flavor (`arm64` vs `amd64`).
pub fn detect_arch_kernel_flavor(arch: &str) -> &'static str {
    match arch {
        "aarch64" => "arm64",
        _ => "amd64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const FIXTURE_PASSWD: &str = "\
root:x:0:0:root:/root:/bin/bash
daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin
nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin
alice:x:1000:1000:Alice:/home/alice:/bin/bash
svc:x:1001:1001:Service:/home/svc:/usr/sbin/nologin
bob:x:1002:1002:Bob:/home/bob:/usr/bin/fish
";

    #[test]
    fn passwd_and_user_helpers() {
        let entries = parse_passwd(FIXTURE_PASSWD);
        assert_eq!(entries.len(), 6);
        assert_eq!(normal_user(&entries).as_deref(), Some("alice"));
        assert_eq!(sot_user(&entries), "alice");

        let real = all_real_users_with_dir_check(&entries, |p| {
            p == Path::new("/home/alice") || p == Path::new("/home/bob")
        });
        assert_eq!(real, vec!["root", "alice", "bob"]);

        assert_eq!(initial_user(Some("alice")), "alice");
        assert_eq!(initial_user(Some("root")), "root");
        assert_eq!(initial_user(None), "root");

        assert_eq!(
            resolve_runtime_home(1000, Some("/custom"), &entries),
            PathBuf::from("/custom")
        );
        assert_eq!(
            resolve_runtime_home(0, None, &entries),
            PathBuf::from("/root")
        );
        assert_eq!(
            resolve_runtime_home(1000, None, &entries),
            PathBuf::from("/home/alice")
        );
        assert_eq!(
            resolve_runtime_home(9999, None, &entries),
            PathBuf::from("/tmp")
        );
    }

    #[test]
    fn detect_debian_major_parses_versions() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("debian_version");
        fs::write(&p, "12.8\n").unwrap();
        assert_eq!(detect_debian_major(&p), Some(12));

        fs::write(&p, "trixie/sid\n").unwrap();
        assert_eq!(detect_debian_major(&p), Some(13));

        fs::write(&p, "forky/sid\n").unwrap();
        assert_eq!(detect_debian_major(&p), Some(14));

        fs::write(&p, "sid\n").unwrap();
        assert_eq!(detect_debian_major(&p), Some(13));

        fs::write(&p, "unknown-release\n").unwrap();
        assert_eq!(detect_debian_major(&p), None);
    }

    #[test]
    fn real_command_runner_executes_and_times_out() {
        let runner = RealCommandRunner;
        let out = runner
            .run("sh", &["-c", "printf 'ok'"], Duration::from_secs(5))
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "ok");

        let timed = runner
            .run("sh", &["-c", "sleep 2"], Duration::from_millis(100))
            .unwrap();
        assert!(timed.timed_out);
        assert!(!timed.success());
    }

    #[test]
    fn command_runner_timeout_with_grandchild_holding_stdout_returns_promptly() {
        let runner = RealCommandRunner;
        let start = std::time::Instant::now();
        let timed = runner
            .run(
                "sh",
                &["-c", "(sleep 30) & sleep 30"],
                Duration::from_millis(200),
            )
            .unwrap();
        let elapsed = start.elapsed();
        assert!(timed.timed_out);
        assert!(!timed.success());
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout hung waiting on grandchild pipe: {elapsed:?}"
        );
    }

    #[test]
    fn command_runner_run_interactive_and_mock() {
        let real = RealCommandRunner;
        assert_eq!(real.run_interactive("sh", &["-c", "exit 7"]).unwrap(), 7);

        let mock = MockCommandRunner::new();
        mock.on_exact(
            "passwd",
            &["alice"],
            CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        assert_eq!(mock.run_interactive("passwd", &["alice"]).unwrap(), 0);
        assert_eq!(
            mock.interactive_invocations(),
            vec![("passwd".to_string(), vec!["alice".to_string()])]
        );
    }

    #[test]
    fn real_command_runner_env_and_stdin() {
        let runner = RealCommandRunner;
        let out = runner
            .run_with_env(
                "sh",
                &["-c", "printf '%s' \"$DEBOPTY_TEST_ENV\""],
                &[("DEBOPTY_TEST_ENV", "wave2")],
                Duration::from_secs(5),
            )
            .unwrap();
        assert!(out.success());
        assert_eq!(out.stdout, "wave2");

        let out_in = runner
            .run_with_stdin("cat", &[], "alice:secret\n", Duration::from_secs(5))
            .unwrap();
        assert!(out_in.success());
        assert_eq!(out_in.stdout, "alice:secret\n");
    }

    #[test]
    fn set_conf_value_updates_uncomments_appends_preserves_mode_and_validates() {
        let tmp = TempDir::new().unwrap();
        let conf = tmp.path().join("sshd_config");
        fs::write(&conf, "#Port 22\nPermitRootLogin yes\n#UsePAM\n").unwrap();
        fs::set_permissions(&conf, fs::Permissions::from_mode(0o644)).unwrap();

        set_conf_value(&conf, "Port", "45678", " ").unwrap();
        set_conf_value(&conf, "PermitRootLogin", "no", " ").unwrap();
        set_conf_value(&conf, "UsePAM", "", "").unwrap();
        set_conf_value(&conf, "PasswordAuthentication", "no", " ").unwrap();

        let got = fs::read_to_string(&conf).unwrap();
        assert_eq!(
            got,
            "Port 45678\nPermitRootLogin no\nUsePAM\nPasswordAuthentication no\n"
        );
        assert_eq!(
            fs::metadata(&conf).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // Handles `# Key = val`, `Key = val`, deduplicates, and preserves custom mode 0o640
        let sysd = tmp.path().join("system.conf");
        fs::write(
            &sysd,
            "[Manager]\n# DefaultLimitNOFILE = 1024\nDefaultLimitNOFILE = 2048\n",
        )
        .unwrap();
        fs::set_permissions(&sysd, fs::Permissions::from_mode(0o640)).unwrap();
        set_conf_value(&sysd, "DefaultLimitNOFILE", "1048576", "=").unwrap();
        assert_eq!(
            fs::read_to_string(&sysd).unwrap(),
            "[Manager]\nDefaultLimitNOFILE=1048576\n"
        );
        assert_eq!(
            fs::metadata(&sysd).unwrap().permissions().mode() & 0o777,
            0o640
        );

        // New file defaults to 0o644 (never 0o600)
        let fresh = tmp.path().join("fresh.conf");
        set_conf_value(&fresh, "Foo", "bar", "=").unwrap();
        assert_eq!(
            fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o644
        );

        // Rejects CR/LF/NUL injection
        assert!(set_conf_value(&conf, "Port", "22\nPermitRootLogin yes", " ").is_err());
        assert!(set_conf_value(&conf, "Port", "22\r", " ").is_err());
        assert!(set_conf_value(&conf, "Port", "22\0", " ").is_err());
    }

    #[test]
    fn safe_apt_install_only_installs_missing_and_reinstalls_rc_pkgs() {
        let runner = MockCommandRunner::new();
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "curl"],
            CmdOutput {
                code: 0,
                stdout: "install ok installed".to_string(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "jq"],
            CmdOutput {
                code: 1,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        // Removed-but-unpurged (`rc`) package returns exit 0 with `deinstall ok config-files`
        runner.on_exact(
            "dpkg-query",
            &["-W", "-f=${Status}", "tmux"],
            CmdOutput {
                code: 0,
                stdout: "deinstall ok config-files".to_string(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        runner.on_prog(
            "apt-get",
            CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );

        let installed = safe_apt_install(&["curl", "jq", "tmux"], &runner).unwrap();
        assert_eq!(installed, vec!["jq", "tmux"]);
        let env_calls = runner.env_invocations();
        assert_eq!(env_calls.len(), 2);
        assert_eq!(env_calls[0].1, vec!["update", "-yq"]);
        assert_eq!(env_calls[1].1, vec!["install", "-yq", "jq", "tmux"]);
        assert_eq!(
            env_calls[1].2,
            vec![("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string())]
        );

        // All already installed -> no apt-get call
        let runner2 = MockCommandRunner::new();
        runner2.on_prog(
            "dpkg-query",
            CmdOutput {
                code: 0,
                stdout: "install ok installed".to_string(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        let installed2 = safe_apt_install(&["curl"], &runner2).unwrap();
        assert!(installed2.is_empty());
        assert!(runner2.env_invocations().is_empty());
    }

    #[test]
    fn mem_total_and_arch_flavor_helpers() {
        let tmp = TempDir::new().unwrap();
        let meminfo = tmp.path().join("meminfo");
        fs::write(
            &meminfo,
            "MemTotal:        2097152 kB\nMemFree:          512000 kB\n",
        )
        .unwrap();
        assert_eq!(read_mem_total_mb(&meminfo), Some(2048));
        assert_eq!(detect_arch_kernel_flavor("aarch64"), "arm64");
        assert_eq!(detect_arch_kernel_flavor("x86_64"), "amd64");
    }
}
