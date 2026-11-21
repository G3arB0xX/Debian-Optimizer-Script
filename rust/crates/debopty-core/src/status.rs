use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::ProjectConfig;
use crate::sys::{self, CommandRunner};
use crate::version::{self, VersionIdentity};

pub const TRACKED_TOOLS: &[&str] = &[
    "fish",
    "micro",
    "yazi",
    "lego",
    "tmux",
    "podman",
    "rustc",
    "go",
    "caddy",
    "derper",
    "realm",
    "ferron",
    "xray",
    "warp-cli",
    "usque",
    "easytier-core",
    "tailscale",
    "freship",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub version_display: String,
    pub is_cn_region: bool,
    pub base_optimized: bool,
    pub ip_forward: bool,
    pub podman_gc_enabled: bool,
    pub tools: BTreeMap<String, bool>,
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

fn find_in_path(cmd: &str, path_dirs: &[PathBuf]) -> bool {
    path_dirs
        .iter()
        .any(|dir| is_executable_file(&dir.join(cmd)))
}

fn check_tool_installed(
    cmd: &str,
    path_dirs: &[PathBuf],
    home_dirs: &[PathBuf],
    root_prefix: &Path,
) -> bool {
    if find_in_path(cmd, path_dirs) {
        return true;
    }
    let dir_name = cmd.strip_suffix("-core").unwrap_or(cmd);
    let fixed_candidates = [
        root_prefix.join("usr/bin").join(cmd),
        root_prefix.join("usr/local/bin").join(cmd),
        root_prefix.join("opt").join(cmd).join(cmd),
        root_prefix.join("opt").join(dir_name).join(cmd),
    ];
    if fixed_candidates.iter().any(|p| p.is_file()) {
        return true;
    }
    if cmd == "freship"
        && root_prefix
            .join("opt/freship/core/freship_core.sh")
            .is_file()
    {
        return true;
    }
    if matches!(cmd, "rust" | "rustc" | "cargo") {
        if root_prefix.join("root/.cargo/bin/rustc").is_file() {
            return true;
        }
        if home_dirs
            .iter()
            .any(|h| h.join(".cargo/bin/rustc").is_file())
        {
            return true;
        }
    }
    false
}

/// Probe system and tool installation status matching `scripts/tui.sh::get_status` and `get_ip_forward_status`.
pub fn probe_status(
    cfg: &ProjectConfig,
    id: &VersionIdentity,
    runner: &dyn CommandRunner,
) -> StatusSnapshot {
    let path_dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default();

    let mut home_dirs: Vec<PathBuf> = Vec::new();
    if let Some(h) = env::var_os("HOME").filter(|h| !h.is_empty()) {
        home_dirs.push(PathBuf::from(h));
    }
    if let Ok(passwd) = fs::read_to_string("/etc/passwd") {
        let entries = sys::parse_passwd(&passwd);
        if let Some(norm) = sys::normal_user(&entries)
            && let Some(entry) = entries.into_iter().find(|e| e.name == norm)
        {
            home_dirs.push(entry.home);
        }
    }

    probe_status_with_env(cfg, id, runner, &path_dirs, &home_dirs, Path::new("/"))
}

pub fn probe_status_with_env(
    cfg: &ProjectConfig,
    id: &VersionIdentity,
    runner: &dyn CommandRunner,
    path_dirs: &[PathBuf],
    home_dirs: &[PathBuf],
    root_prefix: &Path,
) -> StatusSnapshot {
    let ip_forward = runner
        .run(
            "sysctl",
            &["-n", "net.ipv4.ip_forward"],
            Duration::from_secs(2),
        )
        .map(|o| o.success() && o.stdout.trim() == "1")
        .unwrap_or(false);

    let mut tools = BTreeMap::new();
    for &tool in TRACKED_TOOLS {
        let present = check_tool_installed(tool, path_dirs, home_dirs, root_prefix);
        tools.insert(tool.to_string(), present);
    }

    StatusSnapshot {
        version_display: version::format_display(id),
        is_cn_region: cfg.is_cn_region_enabled(),
        base_optimized: cfg.base_optimized,
        ip_forward,
        podman_gc_enabled: cfg.podman_gc_enabled,
        tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{CmdOutput, MockCommandRunner};
    use tempfile::TempDir;

    #[test]
    fn probe_status_detects_tools_ip_forward_and_serializes_json() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        fs::create_dir_all(root.join("usr/local/bin")).unwrap();
        fs::write(root.join("usr/local/bin/fish"), "#!/bin/sh\n").unwrap();

        fs::create_dir_all(root.join("opt/easytier")).unwrap();
        fs::write(root.join("opt/easytier/easytier-core"), "").unwrap();

        fs::create_dir_all(root.join("opt/freship/core")).unwrap();
        fs::write(root.join("opt/freship/core/freship_core.sh"), "").unwrap();

        let fake_home = root.join("home/alice");
        fs::create_dir_all(fake_home.join(".cargo/bin")).unwrap();
        fs::write(fake_home.join(".cargo/bin/rustc"), "").unwrap();

        let runner = MockCommandRunner::new();
        runner.on_exact(
            "sysctl",
            &["-n", "net.ipv4.ip_forward"],
            CmdOutput {
                code: 0,
                stdout: "1\n".to_string(),
                stderr: String::new(),
                timed_out: false,
            },
        );

        let cfg = ProjectConfig {
            is_cn_region: "true".to_string(),
            base_optimized: true,
            podman_gc_enabled: true,
            ..ProjectConfig::default()
        };
        let id = VersionIdentity {
            semver: Some("1.0.7".to_string()),
            calver: Some("26.11.21".to_string()),
            build: "231e05eb".to_string(),
        };

        let snap = probe_status_with_env(&cfg, &id, &runner, &[], &[fake_home], root);
        assert_eq!(snap.version_display, "1.0.7 (231e05eb)");
        assert!(snap.is_cn_region);
        assert!(snap.base_optimized);
        assert!(snap.ip_forward);
        assert!(snap.podman_gc_enabled);
        assert_eq!(snap.tools.len(), TRACKED_TOOLS.len());
        assert_eq!(snap.tools.get("fish"), Some(&true));
        assert_eq!(snap.tools.get("easytier-core"), Some(&true));
        assert_eq!(snap.tools.get("freship"), Some(&true));
        assert_eq!(snap.tools.get("rustc"), Some(&true));
        assert_eq!(snap.tools.get("xray"), Some(&false));

        let json = serde_json::to_string(&snap).unwrap();
        assert!(json.contains("\"version_display\":\"1.0.7 (231e05eb)\""));
        assert!(json.contains("\"ip_forward\":true"));
    }
}
