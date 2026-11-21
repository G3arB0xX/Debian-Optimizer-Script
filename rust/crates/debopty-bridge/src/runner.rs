use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use debopty_core::config::{ConfigStore, DEFAULT_CONFIG_PATH, ProjectConfig};

use crate::actions::{BashAction, BashDispatch, BashFunctionSpec};
use crate::error::{BridgeError, Result};

pub const DEFAULT_INSTALL_DIR: &str = "/opt/debopty";

/// Coexistence bridge for safely executing unported Bash modules from Rust.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashBridge {
    script_dir: PathBuf,
    install_dir: PathBuf,
    config_file: PathBuf,
}

impl BashBridge {
    pub fn new(
        script_dir: impl Into<PathBuf>,
        install_dir: impl Into<PathBuf>,
        config_file: impl Into<PathBuf>,
    ) -> Self {
        Self {
            script_dir: script_dir.into(),
            install_dir: install_dir.into(),
            config_file: config_file.into(),
        }
    }

    pub fn script_dir(&self) -> &Path {
        &self.script_dir
    }

    pub fn install_dir(&self) -> &Path {
        &self.install_dir
    }

    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// Discovers the Bash script directory in priority order:
    /// 1. Explicit `explicit_script_dir` (must be valid if supplied)
    /// 2. `DEBOPTY_SCRIPT_DIR` environment variable
    /// 3. `/opt/debopty`
    /// 4. Current working directory (or its ancestor repo root containing `deb_optimizer.sh`)
    pub fn discover(
        explicit_script_dir: Option<&Path>,
        config_file: Option<&Path>,
    ) -> Result<Self> {
        let install_dir = env::var_os("INSTALL_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_INSTALL_DIR));

        let resolved_config = config_file
            .map(Path::to_path_buf)
            .or_else(|| {
                env::var_os("CONFIG_FILE")
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));

        if let Some(dir) = explicit_script_dir {
            if let Some(resolved) = resolve_valid_script_dir(dir) {
                return Ok(Self::new(resolved, install_dir, resolved_config));
            }
            return Err(BridgeError::ScriptDirNotFound(format!(
                "explicit path '{}' does not contain scripts/common.sh, deb_optimizer.sh, or archive/legacy-bash/",
                dir.display()
            )));
        }

        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(env_dir) = env::var_os("DEBOPTY_SCRIPT_DIR").filter(|v| !v.is_empty()) {
            candidates.push(PathBuf::from(env_dir));
        }
        candidates.push(PathBuf::from(DEFAULT_INSTALL_DIR));
        if let Ok(cwd) = env::current_dir() {
            for ancestor in cwd.ancestors() {
                candidates.push(ancestor.to_path_buf());
            }
        }

        Self::discover_from_candidates(&candidates, install_dir, resolved_config)
    }

    /// Deterministic discovery helper for testing without mutating process env.
    pub fn discover_from_candidates(
        candidates: &[PathBuf],
        install_dir: PathBuf,
        config_file: PathBuf,
    ) -> Result<Self> {
        for candidate in candidates {
            if let Some(resolved) = resolve_valid_script_dir(candidate) {
                return Ok(Self::new(resolved, install_dir, config_file));
            }
        }
        Err(BridgeError::ScriptDirNotFound(
            "checked DEBOPTY_SCRIPT_DIR, /opt/debopty, working directory, and archive/legacy-bash"
                .to_string(),
        ))
    }

    /// Builds a strict, shell-injection-free `Command` for executing `action`.
    ///
    /// Runtime paths are passed exclusively via environment variables (`SCRIPT_DIR`,
    /// `INSTALL_DIR`, `CONFIG_FILE`), never interpolated into shell source strings.
    pub fn build_command(
        &self,
        action: &BashAction,
        extra_env: &[(&str, &str)],
    ) -> Result<Command> {
        let mut cmd = Command::new("bash");
        match action.dispatch() {
            BashDispatch::Entrypoint(rel_script) => {
                let entrypoint = self.script_dir.join(rel_script);
                if !entrypoint.is_file() {
                    return Err(BridgeError::MissingScript { path: entrypoint });
                }
                cmd.arg("--").arg(&entrypoint);
            }
            BashDispatch::Function(spec) => {
                let common_path = self.script_dir.join("scripts/common.sh");
                if !common_path.is_file() {
                    return Err(BridgeError::MissingScript { path: common_path });
                }
                let target_path = self.script_dir.join(spec.module_rel_path);
                if !target_path.is_file() {
                    return Err(BridgeError::MissingScript { path: target_path });
                }
                let snippet = build_static_function_snippet(spec);
                cmd.args(["-euo", "pipefail", "-c", &snippet]);
            }
        }

        for (key, value) in extra_env {
            cmd.env(key, value);
        }

        // Always pin core bridge paths and engine guards AFTER `extra_env` so callers
        // cannot override bridge invariants or inherit `DEBOPTY_ENGINE=rust`.
        cmd.env("SCRIPT_DIR", &self.script_dir)
            .env("INSTALL_DIR", &self.install_dir)
            .env("CONFIG_FILE", &self.config_file)
            .env("IN_TUI", "true")
            .env("DEBOPTY_FROM_RUST", "1")
            .env("DEBOPTY_ENGINE", "bash");

        cmd.stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        Ok(cmd)
    }

    /// Executes `action` with inherited TTY stdio and returns the child exit code.
    pub fn execute(&self, action: &BashAction, extra_env: &[(&str, &str)]) -> Result<i32> {
        let mut cmd = self.build_command(action, extra_env)?;
        let status = cmd.status()?;
        Ok(status.code().unwrap_or(1))
    }

    /// Executes `action` and reloads `ProjectConfig` from `self.config_file` afterwards.
    pub fn execute_and_reload_config(
        &self,
        action: &BashAction,
        extra_env: &[(&str, &str)],
    ) -> Result<(i32, ProjectConfig)> {
        let code = self.execute(action, extra_env)?;
        let cfg = ConfigStore::new(&self.config_file).load()?;
        Ok((code, cfg))
    }
}

fn is_valid_script_dir(dir: &Path) -> bool {
    dir.join("scripts/common.sh").is_file() || dir.join("deb_optimizer.sh").is_file()
}

fn resolve_valid_script_dir(dir: &Path) -> Option<PathBuf> {
    if is_valid_script_dir(dir) {
        return Some(dir.to_path_buf());
    }
    let archived = dir.join("archive/legacy-bash");
    if is_valid_script_dir(&archived) {
        return Some(archived);
    }
    None
}

/// Constructs the Bash `-c` payload strictly from compile-time static identifiers.
fn build_static_function_snippet(spec: BashFunctionSpec) -> String {
    debug_assert!(is_safe_static_rel_path(spec.module_rel_path));
    debug_assert!(is_safe_static_fn_name(spec.function_name));

    let mut lines = vec![
        r#"source "${SCRIPT_DIR}/scripts/common.sh""#.to_string(),
        r#"if declare -f ensure_runtime_home >/dev/null 2>&1; then ensure_runtime_home; fi"#
            .to_string(),
        r#"if declare -f load_project_config >/dev/null 2>&1; then load_project_config; fi"#
            .to_string(),
        r#"export PATH="${PATH}:/usr/local/go/bin:${HOME:-/root}/go/bin""#.to_string(),
    ];

    // Load optional core helper modules when present so cross-module calls work identically
    // to `deb_optimizer.sh` while allowing minimal fixtures in isolated tests.
    for core_mod in [
        "scripts/network.sh",
        "scripts/system.sh",
        "scripts/security.sh",
        "scripts/tui.sh",
    ] {
        if core_mod != spec.module_rel_path {
            lines.push(format!(
                r#"if [[ -f "${{SCRIPT_DIR}}/{core_mod}" ]]; then source "${{SCRIPT_DIR}}/{core_mod}"; fi"#
            ));
        }
    }

    if spec.module_rel_path != "scripts/common.sh" {
        lines.push(format!(
            r#"source "${{SCRIPT_DIR}}/{}""#,
            spec.module_rel_path
        ));
    }

    lines.push(spec.function_name.to_string());
    lines.join("\n")
}

fn is_safe_static_rel_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'.'))
}

fn is_safe_static_fn_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}
