use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

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

        let mut child = cmd.spawn().map_err(|e| CoreError::CommandFailed {
            prog: prog.to_string(),
            message: e.to_string(),
        })?;

        let stdin_handle = if let Some(data) = stdin_data {
            let owned = data.as_bytes().to_vec();
            let mut stdin_pipe = child.stdin.take();
            Some(thread::spawn(move || {
                if let Some(ref mut pipe) = stdin_pipe {
                    let _ = pipe.write_all(&owned);
                    let _ = pipe.flush();
                }
            }))
        } else {
            None
        };

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

        if let Some(h) = stdin_handle {
            let _ = h.join();
        }
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

    pub fn env_invocations(&self) -> Vec<EnvInvocation> {
        self.env_invocations.lock().unwrap().clone()
    }

    pub fn stdin_invocations(&self) -> Vec<StdinInvocation> {
        self.stdin_invocations.lock().unwrap().clone()
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
            .unwrap()
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
        self.stdin_invocations.lock().unwrap().push((
            prog.to_string(),
            arg_vec,
            stdin_data.to_string(),
        ));
        self.run(prog, args, timeout)
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

/// Atomically update or append a key-value (or flag) line in a config file.
///
/// Matches `^#?<key><sep>` (or `^#?<key>\s*$` when `value.is_empty() && sep.is_empty()`),
/// replacing matched line(s) in place or appending a new line if absent.
pub fn set_conf_value(path: &Path, key: &str, value: &str, sep: &str) -> Result<()> {
    let existing = if path.is_file() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    let (pattern, replacement) = if value.is_empty() && sep.is_empty() {
        (format!(r"^#?{}\s*$", regex::escape(key)), key.to_string())
    } else {
        (
            format!(r"^#?{}{}", regex::escape(key), regex::escape(sep)),
            format!("{key}{sep}{value}"),
        )
    };

    let re = Regex::new(&pattern)
        .map_err(|e| CoreError::InvalidConfig(format!("invalid conf pattern: {e}")))?;

    let mut updated = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in existing.lines() {
        if re.is_match(line) {
            out_lines.push(replacement.clone());
            updated = true;
        } else {
            out_lines.push(line.to_string());
        }
    }
    if !updated {
        out_lines.push(replacement);
    }

    let mut rendered = out_lines.join("\n");
    rendered.push('\n');

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut tmp = NamedTempFile::new_in(parent)?;
    tmp.write_all(rendered.as_bytes())?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| CoreError::Io(e.error))?;
    Ok(())
}

/// Install only missing APT packages non-interactively; returns the list of installed packages.
pub fn safe_apt_install(pkgs: &[&str], runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let mut missing: Vec<&str> = Vec::new();
    for &pkg in pkgs {
        let installed = runner
            .run("dpkg", &["-s", pkg], Duration::from_secs(15))
            .map(|o| o.success())
            .unwrap_or(false);
        if !installed {
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
    fn set_conf_value_updates_uncomments_and_appends() {
        let tmp = TempDir::new().unwrap();
        let conf = tmp.path().join("sshd_config");
        fs::write(&conf, "#Port 22\nPermitRootLogin yes\n#UsePAM\n").unwrap();

        set_conf_value(&conf, "Port", "45678", " ").unwrap();
        set_conf_value(&conf, "PermitRootLogin", "no", " ").unwrap();
        set_conf_value(&conf, "UsePAM", "", "").unwrap();
        set_conf_value(&conf, "PasswordAuthentication", "no", " ").unwrap();

        let got = fs::read_to_string(&conf).unwrap();
        assert_eq!(
            got,
            "Port 45678\nPermitRootLogin no\nUsePAM\nPasswordAuthentication no\n"
        );
    }

    #[test]
    fn safe_apt_install_only_installs_missing_pkgs() {
        let runner = MockCommandRunner::new();
        runner.on_exact(
            "dpkg",
            &["-s", "curl"],
            CmdOutput {
                code: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        runner.on_exact(
            "dpkg",
            &["-s", "jq"],
            CmdOutput {
                code: 1,
                stdout: String::new(),
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

        let installed = safe_apt_install(&["curl", "jq"], &runner).unwrap();
        assert_eq!(installed, vec!["jq"]);
        let env_calls = runner.env_invocations();
        assert_eq!(env_calls.len(), 2);
        assert_eq!(env_calls[0].1, vec!["update", "-yq"]);
        assert_eq!(env_calls[1].1, vec!["install", "-yq", "jq"]);
        assert_eq!(
            env_calls[1].2,
            vec![("DEBIAN_FRONTEND".to_string(), "noninteractive".to_string())]
        );

        // All already installed -> no apt-get call
        let runner2 = MockCommandRunner::new();
        runner2.on_prog(
            "dpkg",
            CmdOutput {
                code: 0,
                stdout: String::new(),
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
