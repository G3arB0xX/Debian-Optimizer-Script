use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

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
}

#[derive(Debug, Default, Clone, Copy)]
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, prog: &str, args: &[&str], timeout: Duration) -> Result<CmdOutput> {
        let mut child = Command::new(prog)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| CoreError::CommandFailed {
                prog: prog.to_string(),
                message: e.to_string(),
            })?;

        let mut stdout_pipe = child.stdout.take();
        let mut stderr_pipe = child.stderr.take();

        let stdout_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(ref mut pipe) = stdout_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        });
        let stderr_handle = thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(ref mut pipe) = stderr_pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        });

        let (code, timed_out) = match child.wait_timeout(timeout)? {
            Some(status) => (status.code().unwrap_or(-1), false),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                (-1, true)
            }
        };

        let stdout = stdout_handle.join().unwrap_or_default();
        let stderr = stderr_handle.join().unwrap_or_default();

        Ok(CmdOutput {
            code,
            stdout,
            stderr,
            timed_out,
        })
    }
}

#[derive(Debug, Default)]
pub struct MockCommandRunner {
    responses: Mutex<HashMap<(String, Vec<String>), CmdOutput>>,
    prog_defaults: Mutex<HashMap<String, CmdOutput>>,
    invocations: Mutex<Vec<(String, Vec<String>)>>,
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
        self.responses.lock().unwrap().insert(key, output);
    }

    pub fn on_prog(&self, prog: &str, output: CmdOutput) {
        self.prog_defaults
            .lock()
            .unwrap()
            .insert(prog.to_string(), output);
    }

    pub fn invocations(&self) -> Vec<(String, Vec<String>)> {
        self.invocations.lock().unwrap().clone()
    }
}

impl CommandRunner for MockCommandRunner {
    fn run(&self, prog: &str, args: &[&str], _timeout: Duration) -> Result<CmdOutput> {
        let arg_vec: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
        self.invocations
            .lock()
            .unwrap()
            .push((prog.to_string(), arg_vec.clone()));

        if let Some(out) = self
            .responses
            .lock()
            .unwrap()
            .get(&(prog.to_string(), arg_vec))
        {
            return Ok(out.clone());
        }
        if let Some(out) = self.prog_defaults.lock().unwrap().get(prog) {
            return Ok(out.clone());
        }
        Err(CoreError::CommandFailed {
            prog: prog.to_string(),
            message: "command not mocked".to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswdEntry {
    pub name: String,
    pub uid: u32,
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
        entries.push(PasswdEntry {
            name: fields[0].to_string(),
            uid,
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

/// Detect Debian major version from `/etc/debian_version` (e.g. `"12.5"` -> `Some(12)`).
pub fn detect_debian_major(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let first = text.trim().split('.').next()?;
    first.parse::<u32>().ok()
}

pub fn current_euid() -> u32 {
    // SAFETY: `geteuid` is always safe to call on Unix/Linux.
    unsafe { libc::geteuid() }
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
}
