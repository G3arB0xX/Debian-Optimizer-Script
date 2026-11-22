use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::apps::self_manage::sha256_hex;
use crate::credentials::{self, CRED_DIR_MODE, CRED_FILE_MODE, CredentialAuditReport};
use crate::diag::{self, LogLevel};
use crate::error::{CoreError, Result};
use crate::reconcile::backup;
use crate::sys::{self, CommandRunner};
use crate::telemetry;
use crate::template::TemplateEngine;
use crate::version;

pub const DEFAULT_WORKSPACE_DIR: &str = "/var/lib/debopty/agent";
pub const WORKSPACE_SCHEMA_VERSION: &str = "1";
pub const AUDIT_LOG_REL: &str = ".agents/logs/audit.jsonl";
pub const CHECKPOINTS_DIR_REL: &str = ".agents/checkpoints";
pub const CREDENTIALS_DIR_REL: &str = ".agents/credentials";
pub const MEMORY_FILE_REL: &str = "MEMORY.md";
pub const HOST_PROFILE_REL: &str = ".agents/memory/host-profile.md";
pub const MANAGED_START_MARKER: &str = "<!-- managed-by-debopty:start -->";
pub const MANAGED_END_MARKER: &str = "<!-- managed-by-debopty:end -->";

const DIAG_TARGET: &str = "debopty::agent_workspace";
const WORKSPACE_MANIFEST_REL: &str = ".agents/workspace.json";
const LOGS_DIR_REL: &str = ".agents/logs";
const DEFAULT_DEBOPTY_BIN: &str = "/usr/local/bin/debopty";

const MANAGED_DOC_TEMPLATES: &[(&str, &str, u32)] = &[
    ("templates/agent/AGENTS.md.tmpl", "AGENTS.md", 0o644),
    ("templates/agent/README.md.tmpl", "README.md", 0o644),
    ("templates/agent/INDEX.md.tmpl", ".agents/INDEX.md", 0o644),
    (
        "templates/agent/CONSTRAINTS.md.tmpl",
        ".agents/CONSTRAINTS.md",
        0o644,
    ),
    (
        "templates/agent/constraints/INDEX.md.tmpl",
        ".agents/constraints/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/constraints/README.md.tmpl",
        ".agents/constraints/README.md",
        0o644,
    ),
    (
        "templates/agent/constraints/00_vps_general_and_docs.md.tmpl",
        ".agents/constraints/00_vps_general_and_docs.md",
        0o644,
    ),
    (
        "templates/agent/constraints/10_system_change_and_style.md.tmpl",
        ".agents/constraints/10_system_change_and_style.md",
        0o644,
    ),
    (
        "templates/agent/constraints/20_security_and_credentials.md.tmpl",
        ".agents/constraints/20_security_and_credentials.md",
        0o644,
    ),
    (
        "templates/agent/constraints/30_atomic_ops_and_audit.md.tmpl",
        ".agents/constraints/30_atomic_ops_and_audit.md",
        0o644,
    ),
    (
        "templates/agent/memory/INDEX.md.tmpl",
        ".agents/memory/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/memory/README.md.tmpl",
        ".agents/memory/README.md",
        0o644,
    ),
];

const HOST_PROFILE_TEMPLATE: &str = "templates/agent/memory/host-profile.md.tmpl";
const MEMORY_SEED_TEMPLATE: &str = "templates/agent/MEMORY.md.tmpl";

const AGENT_ONLY_FILES: &[&str] = &[
    "AGENTS.md",
    ".agents/INDEX.md",
    ".agents/CONSTRAINTS.md",
    ".agents/constraints/INDEX.md",
    ".agents/memory/INDEX.md",
];

const HUMAN_FACING_FILES: &[&str] = &[
    "README.md",
    "MEMORY.md",
    ".agents/constraints/README.md",
    ".agents/constraints/00_vps_general_and_docs.md",
    ".agents/constraints/10_system_change_and_style.md",
    ".agents/constraints/20_security_and_credentials.md",
    ".agents/constraints/30_atomic_ops_and_audit.md",
    ".agents/memory/README.md",
    ".agents/memory/host-profile.md",
];

const DATE_BAN_FILES: &[&str] = &[
    ".agents/CONSTRAINTS.md",
    ".agents/constraints/INDEX.md",
    ".agents/constraints/README.md",
    ".agents/constraints/00_vps_general_and_docs.md",
    ".agents/constraints/10_system_change_and_style.md",
    ".agents/constraints/20_security_and_credentials.md",
    ".agents/constraints/30_atomic_ops_and_audit.md",
];

const ALLOWED_XML_TAGS: &[&str] = &[
    "guide",
    "constraints_index",
    "index",
    "workflow",
    "skill",
    "card",
    "task",
    "protocol",
];

#[derive(Debug, Clone)]
pub struct AgentWorkspaceOptions {
    pub root_prefix: PathBuf,
    pub workspace_dir: PathBuf,
    pub template_dir: Option<PathBuf>,
    pub force_overwrite_managed: bool,
}

impl AgentWorkspaceOptions {
    pub fn new(root_prefix: impl Into<PathBuf>, workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_prefix: root_prefix.into(),
            workspace_dir: workspace_dir.into(),
            template_dir: None,
            force_overwrite_managed: false,
        }
    }

    pub fn effective_workspace_dir(&self) -> PathBuf {
        if self.root_prefix.as_os_str().is_empty()
            || self.root_prefix == Path::new("/")
            || self.workspace_dir.starts_with(&self.root_prefix)
        {
            self.workspace_dir.clone()
        } else if self.workspace_dir.is_absolute() {
            let s = self.workspace_dir.to_string_lossy();
            self.root_prefix.join(s.trim_start_matches('/'))
        } else {
            self.root_prefix.join(&self.workspace_dir)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InitWorkspaceReport {
    pub workspace_dir: String,
    pub schema_version: String,
    pub debopty_version: String,
    pub created_files: Vec<String>,
    pub updated_files: Vec<String>,
    pub preserved_files: Vec<String>,
    pub unchanged_files: Vec<String>,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointMeta {
    pub checkpoint_id: String,
    pub created_at: String,
    pub label: String,
    pub reason: String,
    pub paths: Vec<String>,
    pub backup_dir: String,
    pub entry_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rolled_back_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceStatusReport {
    pub workspace_dir: String,
    pub initialized: bool,
    pub schema_version: Option<String>,
    pub debopty_version: String,
    pub managed_files_present: usize,
    pub managed_files_expected: usize,
    pub missing_files: Vec<String>,
    pub memory_exists: bool,
    pub memory_lines: usize,
    pub audit_entries: usize,
    pub checkpoint_count: usize,
    pub latest_checkpoint: Option<CheckpointMeta>,
    pub credential_audit: CredentialAuditReport,
    pub healthy: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackReport {
    pub checkpoint_id: String,
    pub restored_entries: usize,
    pub backup_dir: String,
    pub audit_recorded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub ts: String,
    pub action: String,
    pub actor: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub details: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceVerifyReport {
    pub workspace_dir: String,
    pub passed: bool,
    pub checks_run: usize,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceManifest {
    schema_version: String,
    debopty_version: String,
    created_at: String,
    updated_at: String,
    #[serde(default)]
    managed_hashes: BTreeMap<String, String>,
}

fn current_debopty_version() -> String {
    let id = version::compiled_identity();
    id.primary()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.0.0".to_string())
}

fn ensure_dir_mode(path: &Path, mode: u32) -> Result<bool> {
    let existed = path.is_dir();
    if !existed {
        fs::create_dir_all(path)?;
    }
    let current_mode = fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0);
    if !existed || current_mode != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        return Ok(true);
    }
    Ok(false)
}

fn build_template_vars(
    opts: &AgentWorkspaceOptions,
    ws_dir: &Path,
    debopty_version: &str,
    now: &str,
) -> BTreeMap<String, String> {
    let specs = telemetry::probe_system_specs(&opts.root_prefix);
    let mut vars = BTreeMap::new();
    vars.insert(
        "WORKSPACE_DIR".to_string(),
        ws_dir.to_string_lossy().into_owned(),
    );
    vars.insert("DEBOPTY_BIN".to_string(), DEFAULT_DEBOPTY_BIN.to_string());
    vars.insert("DEBOPTY_VERSION".to_string(), debopty_version.to_string());
    vars.insert("HOSTNAME".to_string(), specs.hostname);
    vars.insert("OS_NAME".to_string(), specs.os_name);
    vars.insert("ARCH".to_string(), specs.arch);
    vars.insert(
        "KERNEL".to_string(),
        if specs.kernel.is_empty() {
            "unknown".to_string()
        } else {
            specs.kernel
        },
    );
    vars.insert("UPDATED_AT".to_string(), now.to_string());
    vars
}

fn extract_managed_block(text: &str) -> Option<(usize, usize, &str)> {
    let start = text.find(MANAGED_START_MARKER)?;
    let after_start = start + MANAGED_START_MARKER.len();
    let end_rel = text[after_start..].find(MANAGED_END_MARKER)?;
    let end = after_start + end_rel + MANAGED_END_MARKER.len();
    Some((start, end, &text[start..end]))
}

fn normalize_managed_block_for_compare(block: &str) -> String {
    block
        .lines()
        .filter(|line| !line.trim_start().starts_with("- 最近刷新时间："))
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_safe_identifier(raw: &str, kind: &str) -> Result<()> {
    if raw.is_empty()
        || raw.len() > 96
        || raw == "."
        || raw == ".."
        || raw.contains("..")
        || raw.contains(['/', '\\', '\0', '\r', '\n'])
        || !raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid {kind}: '{raw}'"
        )));
    }
    Ok(())
}

/// Idempotently initialize or non-destructively upgrade the standardized VPS agent workspace.
pub fn init_workspace(opts: &AgentWorkspaceOptions) -> Result<InitWorkspaceReport> {
    let ws_dir = opts.effective_workspace_dir();
    let debopty_version = current_debopty_version();
    let now = diag::utc_rfc3339_now();
    let tpl = TemplateEngine::new(opts.template_dir.clone()).with_root_prefix(&opts.root_prefix);
    let vars = build_template_vars(opts, &ws_dir, &debopty_version, &now);

    let mut changed = false;
    changed |= ensure_dir_mode(&ws_dir, 0o755)?;
    changed |= ensure_dir_mode(&ws_dir.join(".agents"), 0o755)?;
    changed |= ensure_dir_mode(&ws_dir.join(".agents/constraints"), 0o755)?;
    changed |= ensure_dir_mode(&ws_dir.join(".agents/memory"), 0o755)?;
    changed |= ensure_dir_mode(&ws_dir.join(LOGS_DIR_REL), 0o700)?;
    changed |= ensure_dir_mode(&ws_dir.join(CHECKPOINTS_DIR_REL), 0o700)?;
    changed |= ensure_dir_mode(&ws_dir.join(CREDENTIALS_DIR_REL), CRED_DIR_MODE)?;

    let manifest_path = ws_dir.join(WORKSPACE_MANIFEST_REL);
    let existing_manifest: Option<WorkspaceManifest> = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok());
    let mut managed_hashes = existing_manifest
        .as_ref()
        .map(|m| m.managed_hashes.clone())
        .unwrap_or_default();

    let mut created_files = Vec::new();
    let mut updated_files = Vec::new();
    let mut preserved_files = Vec::new();
    let mut unchanged_files = Vec::new();

    // 1. 3-way SHA-256 managed documentation & index templates.
    for &(tpl_rel, rel_dest, mode) in MANAGED_DOC_TEMPLATES {
        let rendered = tpl.render_to_string(tpl_rel, &vars)?;
        let new_hash = sha256_hex(rendered.as_bytes());
        let dest = ws_dir.join(rel_dest);

        if !dest.is_file() {
            sys::write_file_atomic(&dest, rendered.as_bytes(), Some(mode))?;
            managed_hashes.insert(rel_dest.to_string(), new_hash);
            created_files.push(rel_dest.to_string());
            changed = true;
            continue;
        }

        let disk_bytes = fs::read(&dest)?;
        let disk_hash = sha256_hex(&disk_bytes);
        if disk_hash == new_hash {
            let cur_mode = fs::metadata(&dest)?.permissions().mode() & 0o7777;
            if cur_mode != mode {
                fs::set_permissions(&dest, fs::Permissions::from_mode(mode))?;
                changed = true;
            }
            managed_hashes.insert(rel_dest.to_string(), new_hash);
            unchanged_files.push(rel_dest.to_string());
        } else {
            let prev_matches = managed_hashes
                .get(rel_dest)
                .is_some_and(|prev| prev == &disk_hash);
            if opts.force_overwrite_managed || prev_matches {
                sys::write_file_atomic(&dest, rendered.as_bytes(), Some(mode))?;
                managed_hashes.insert(rel_dest.to_string(), new_hash);
                updated_files.push(rel_dest.to_string());
                changed = true;
            } else {
                preserved_files.push(rel_dest.to_string());
            }
        }
    }

    // 2. Managed marker block file: .agents/memory/host-profile.md (custom tail preserved).
    {
        let rendered = tpl.render_to_string(HOST_PROFILE_TEMPLATE, &vars)?;
        let dest = ws_dir.join(HOST_PROFILE_REL);
        if !dest.is_file() {
            sys::write_file_atomic(&dest, rendered.as_bytes(), Some(0o644))?;
            created_files.push(HOST_PROFILE_REL.to_string());
            changed = true;
        } else {
            let disk_text = fs::read_to_string(&dest)?;
            let cur_mode = fs::metadata(&dest)?.permissions().mode() & 0o7777;
            match (
                extract_managed_block(&disk_text),
                extract_managed_block(&rendered),
            ) {
                (Some((d_start, d_end, d_block)), Some((_, _, n_block))) => {
                    if normalize_managed_block_for_compare(d_block)
                        == normalize_managed_block_for_compare(n_block)
                    {
                        if cur_mode != 0o644 {
                            fs::set_permissions(&dest, fs::Permissions::from_mode(0o644))?;
                            changed = true;
                        }
                        unchanged_files.push(HOST_PROFILE_REL.to_string());
                    } else {
                        let mut spliced = String::with_capacity(disk_text.len() + n_block.len());
                        spliced.push_str(&disk_text[..d_start]);
                        spliced.push_str(n_block);
                        spliced.push_str(&disk_text[d_end..]);
                        sys::write_file_atomic(&dest, spliced.as_bytes(), Some(0o644))?;
                        updated_files.push(HOST_PROFILE_REL.to_string());
                        changed = true;
                    }
                }
                _ => {
                    if opts.force_overwrite_managed {
                        sys::write_file_atomic(&dest, rendered.as_bytes(), Some(0o644))?;
                        updated_files.push(HOST_PROFILE_REL.to_string());
                        changed = true;
                    } else {
                        if cur_mode != 0o644 {
                            fs::set_permissions(&dest, fs::Permissions::from_mode(0o644))?;
                            changed = true;
                        }
                        preserved_files.push(HOST_PROFILE_REL.to_string());
                    }
                }
            }
        }
    }

    // 3. Seed-only MEMORY.md (0600, NEVER overwritten once present).
    {
        let dest = ws_dir.join(MEMORY_FILE_REL);
        if !dest.is_file() {
            let rendered = tpl.render_to_string(MEMORY_SEED_TEMPLATE, &vars)?;
            sys::write_file_atomic(&dest, rendered.as_bytes(), Some(CRED_FILE_MODE))?;
            created_files.push(MEMORY_FILE_REL.to_string());
            changed = true;
        } else {
            let cur_mode = fs::metadata(&dest)?.permissions().mode() & 0o7777;
            if cur_mode != CRED_FILE_MODE {
                fs::set_permissions(&dest, fs::Permissions::from_mode(CRED_FILE_MODE))?;
                changed = true;
            }
            preserved_files.push(MEMORY_FILE_REL.to_string());
        }
    }

    // 4. Append-only audit.jsonl (0600, created empty if absent, NEVER truncated).
    {
        let audit_path = ws_dir.join(AUDIT_LOG_REL);
        if !audit_path.is_file() {
            OpenOptions::new()
                .create(true)
                .append(true)
                .mode(CRED_FILE_MODE)
                .open(&audit_path)?;
            fs::set_permissions(&audit_path, fs::Permissions::from_mode(CRED_FILE_MODE))?;
            changed = true;
        } else {
            let cur_mode = fs::metadata(&audit_path)?.permissions().mode() & 0o7777;
            if cur_mode != CRED_FILE_MODE {
                fs::set_permissions(&audit_path, fs::Permissions::from_mode(CRED_FILE_MODE))?;
                changed = true;
            }
        }
    }

    // 5. Persist workspace.json (0600) only when created or changed (keeps re-init 100% idempotent).
    let manifest_needs_write = changed
        || existing_manifest.as_ref().is_none_or(|m| {
            m.schema_version != WORKSPACE_SCHEMA_VERSION
                || m.debopty_version != debopty_version
                || m.managed_hashes != managed_hashes
        });
    if manifest_needs_write {
        let created_at = existing_manifest
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| now.clone());
        let manifest = WorkspaceManifest {
            schema_version: WORKSPACE_SCHEMA_VERSION.to_string(),
            debopty_version: debopty_version.clone(),
            created_at,
            updated_at: now.clone(),
            managed_hashes,
        };
        let mut json = serde_json::to_string_pretty(&manifest)
            .map_err(|e| CoreError::System(format!("serialize workspace manifest failed: {e}")))?;
        json.push('\n');
        sys::write_file_atomic(&manifest_path, json.as_bytes(), Some(CRED_FILE_MODE))?;
        changed = true;
    } else {
        let cur_mode = fs::metadata(&manifest_path)?.permissions().mode() & 0o7777;
        if cur_mode != CRED_FILE_MODE {
            fs::set_permissions(&manifest_path, fs::Permissions::from_mode(CRED_FILE_MODE))?;
            changed = true;
        }
    }

    if changed {
        let entry = AuditEntry {
            ts: now,
            action: "workspace-init".to_string(),
            actor: "debopty".to_string(),
            status: "ok".to_string(),
            checkpoint_id: None,
            summary: format!(
                "initialized/updated agent workspace (created={}, updated={}, preserved={})",
                created_files.len(),
                updated_files.len(),
                preserved_files.len()
            ),
            paths: Vec::new(),
            details: BTreeMap::from([
                (
                    "schema_version".to_string(),
                    WORKSPACE_SCHEMA_VERSION.to_string(),
                ),
                ("debopty_version".to_string(), debopty_version.clone()),
            ]),
        };
        append_audit_entry(&ws_dir, &entry)?;
        diag::log_event(
            LogLevel::Info,
            DIAG_TARGET,
            None,
            "initialized agent workspace",
            None,
            &[("workspace_dir", &ws_dir.to_string_lossy())],
        );
    }

    Ok(InitWorkspaceReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        schema_version: WORKSPACE_SCHEMA_VERSION.to_string(),
        debopty_version,
        created_files,
        updated_files,
        preserved_files,
        unchanged_files,
        changed,
    })
}

/// Inspect the health, files, checkpoints, audit log, and credential hygiene of a workspace.
pub fn workspace_status(
    opts: &AgentWorkspaceOptions,
    runner: Option<&dyn CommandRunner>,
) -> Result<WorkspaceStatusReport> {
    let ws_dir = opts.effective_workspace_dir();
    let debopty_version = current_debopty_version();

    let manifest_path = ws_dir.join(WORKSPACE_MANIFEST_REL);
    let schema_version = fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_str::<WorkspaceManifest>(&raw).ok())
        .map(|m| m.schema_version);

    let mut expected: Vec<&str> = MANAGED_DOC_TEMPLATES.iter().map(|(_, rel, _)| *rel).collect();
    expected.push(HOST_PROFILE_REL);
    let managed_files_expected = expected.len();

    let mut managed_files_present = 0usize;
    let mut missing_files = Vec::new();
    for rel in expected {
        if ws_dir.join(rel).is_file() {
            managed_files_present += 1;
        } else {
            missing_files.push(rel.to_string());
        }
    }

    let memory_path = ws_dir.join(MEMORY_FILE_REL);
    let memory_exists = memory_path.is_file();
    let memory_lines = if memory_exists {
        fs::read_to_string(&memory_path)
            .map(|s| s.lines().count())
            .unwrap_or(0)
    } else {
        0
    };

    let audit_path = ws_dir.join(AUDIT_LOG_REL);
    let audit_entries = if audit_path.is_file() {
        fs::read_to_string(&audit_path)
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    } else {
        0
    };

    let checkpoints = list_checkpoints(opts)?;
    let checkpoint_count = checkpoints.len();
    let latest_checkpoint = checkpoints.into_iter().next();

    let credential_audit =
        credentials::audit_credentials(&ws_dir.join(CREDENTIALS_DIR_REL), &[], runner);
    let initialized = manifest_path.is_file() && managed_files_present > 0;
    let strict_perms_ok = check_core_runtime_perms(&ws_dir).is_empty();
    let healthy = initialized
        && missing_files.is_empty()
        && memory_exists
        && credential_audit.ok
        && strict_perms_ok;

    Ok(WorkspaceStatusReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        initialized,
        schema_version,
        debopty_version,
        managed_files_present,
        managed_files_expected,
        missing_files,
        memory_exists,
        memory_lines,
        audit_entries,
        checkpoint_count,
        latest_checkpoint,
        credential_audit,
        healthy,
    })
}

/// Create an atomic rollback checkpoint under `.agents/checkpoints/<checkpoint_id>/`.
pub fn create_checkpoint(
    opts: &AgentWorkspaceOptions,
    label: &str,
    reason: &str,
    paths: &[String],
    actor: &str,
) -> Result<CheckpointMeta> {
    validate_safe_identifier(label, "checkpoint label")?;
    let ws_dir = opts.effective_workspace_dir();
    let checkpoints_dir = ws_dir.join(CHECKPOINTS_DIR_REL);
    ensure_dir_mode(&checkpoints_dir, 0o700)?;

    let (run_id, run_dir, manifest) =
        backup::create_snapshot(&opts.root_prefix, &checkpoints_dir, paths)?;

    let clean_reason = diag::redact_sensitive(reason);
    let clean_actor = diag::redact_sensitive(actor);
    let captured_paths: Vec<String> = manifest
        .entries
        .iter()
        .map(|e| format!("/{}", e.original_path))
        .collect();

    let meta = CheckpointMeta {
        checkpoint_id: run_id.clone(),
        created_at: diag::utc_rfc3339_now(),
        label: label.to_string(),
        reason: clean_reason.clone(),
        paths: captured_paths.clone(),
        backup_dir: run_dir.to_string_lossy().into_owned(),
        entry_count: manifest.entries.len(),
        rolled_back_at: None,
    };

    let mut json = serde_json::to_string_pretty(&meta)
        .map_err(|e| CoreError::Reconcile(format!("serialize checkpoint metadata failed: {e}")))?;
    json.push('\n');
    sys::write_file_atomic(
        &run_dir.join("checkpoint.json"),
        json.as_bytes(),
        Some(CRED_FILE_MODE),
    )?;

    let audit = AuditEntry {
        ts: meta.created_at.clone(),
        action: "checkpoint-create".to_string(),
        actor: clean_actor,
        status: "ok".to_string(),
        checkpoint_id: Some(run_id),
        summary: format!(
            "created checkpoint '{label}' ({} entries): {clean_reason}",
            meta.entry_count
        ),
        paths: captured_paths,
        details: BTreeMap::from([("label".to_string(), label.to_string())]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(meta)
}

/// List recorded checkpoints sorted newest-first.
pub fn list_checkpoints(opts: &AgentWorkspaceOptions) -> Result<Vec<CheckpointMeta>> {
    let checkpoints_dir = opts.effective_workspace_dir().join(CHECKPOINTS_DIR_REL);
    if !checkpoints_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut list = Vec::new();
    for entry in fs::read_dir(&checkpoints_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let meta_path = path.join("checkpoint.json");
        if !meta_path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&meta_path)?;
        if let Ok(meta) = serde_json::from_str::<CheckpointMeta>(&raw) {
            list.push(meta);
        }
    }

    list.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.checkpoint_id.cmp(&a.checkpoint_id))
    });
    Ok(list)
}

/// Roll back all paths captured in `checkpoint_id` onto `opts.root_prefix` and record an audit entry.
pub fn rollback_checkpoint(
    opts: &AgentWorkspaceOptions,
    checkpoint_id: &str,
    actor: &str,
) -> Result<RollbackReport> {
    validate_safe_identifier(checkpoint_id, "checkpoint id")?;
    let ws_dir = opts.effective_workspace_dir();
    let run_dir = ws_dir.join(CHECKPOINTS_DIR_REL).join(checkpoint_id);
    if !run_dir.is_dir() || !run_dir.join("manifest.json").is_file() {
        return Err(CoreError::Reconcile(format!(
            "checkpoint not found: '{checkpoint_id}'"
        )));
    }

    let restored_entries = backup::restore_backup(&run_dir, &opts.root_prefix)?;
    let now = diag::utc_rfc3339_now();

    let meta_path = run_dir.join("checkpoint.json");
    let mut paths = Vec::new();
    if meta_path.is_file()
        && let Ok(raw) = fs::read_to_string(&meta_path)
        && let Ok(mut meta) = serde_json::from_str::<CheckpointMeta>(&raw)
    {
        paths = meta.paths.clone();
        meta.rolled_back_at = Some(now.clone());
        if let Ok(mut json) = serde_json::to_string_pretty(&meta) {
            json.push('\n');
            sys::write_file_atomic(&meta_path, json.as_bytes(), Some(CRED_FILE_MODE))?;
        }
    }

    let audit = AuditEntry {
        ts: now,
        action: "checkpoint-rollback".to_string(),
        actor: diag::redact_sensitive(actor),
        status: "rolled_back".to_string(),
        checkpoint_id: Some(checkpoint_id.to_string()),
        summary: format!("rolled back checkpoint '{checkpoint_id}' ({restored_entries} entries)"),
        paths,
        details: BTreeMap::from([("restored_entries".to_string(), restored_entries.to_string())]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(RollbackReport {
        checkpoint_id: checkpoint_id.to_string(),
        restored_entries,
        backup_dir: run_dir.to_string_lossy().into_owned(),
        audit_recorded: true,
    })
}

/// Append a redacted single-line NDJSON entry to `<ws_dir>/.agents/logs/audit.jsonl` (`0600`).
pub fn append_audit_entry(ws_dir: &Path, entry: &AuditEntry) -> Result<()> {
    let logs_dir = ws_dir.join(LOGS_DIR_REL);
    ensure_dir_mode(&logs_dir, 0o700)?;

    let mut clean = entry.clone();
    if clean.ts.trim().is_empty() {
        clean.ts = diag::utc_rfc3339_now();
    }
    let mut val = serde_json::to_value(&clean)
        .map_err(|e| CoreError::System(format!("serialize audit entry failed: {e}")))?;
    diag::redact_json_value(&mut val);
    let line = serde_json::to_string(&val)
        .map_err(|e| CoreError::System(format!("format audit JSON failed: {e}")))?;

    let audit_path = ws_dir.join(AUDIT_LOG_REL);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(CRED_FILE_MODE)
        .open(&audit_path)?;
    writeln!(file, "{line}")?;
    file.flush()?;
    fs::set_permissions(&audit_path, fs::Permissions::from_mode(CRED_FILE_MODE))?;
    Ok(())
}

/// Read up to the most recent `limit` entries (`0` = all) from `<ws_dir>/.agents/logs/audit.jsonl`.
pub fn read_audit_log(ws_dir: &Path, limit: usize) -> Result<Vec<AuditEntry>> {
    let audit_path = ws_dir.join(AUDIT_LOG_REL);
    if !audit_path.is_file() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(&audit_path)?;
    let mut entries = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let entry: AuditEntry = serde_json::from_str(trimmed)
            .map_err(|e| CoreError::InvalidConfig(format!("invalid audit.jsonl entry: {e}")))?;
        entries.push(entry);
    }
    if limit > 0 && entries.len() > limit {
        let start = entries.len() - limit;
        Ok(entries.split_off(start))
    } else {
        Ok(entries)
    }
}

/// Append a redacted note under `## <section>` in `<ws_dir>/MEMORY.md` (`0600`).
pub fn append_memory_note(ws_dir: &Path, section: &str, note: &str) -> Result<bool> {
    let clean_note_raw = note.trim();
    if clean_note_raw.is_empty() {
        return Err(CoreError::ValidationFailed(
            "memory note must not be empty".to_string(),
        ));
    }
    let sec_raw = section.trim();
    if sec_raw.contains(['\r', '\n', '\0']) {
        return Err(CoreError::ValidationFailed(
            "memory section heading must be single-line".to_string(),
        ));
    }
    let eff_section = if sec_raw.is_empty() {
        "重要变更与排障记录".to_string()
    } else {
        diag::redact_sensitive(sec_raw)
    };
    let clean_note = diag::redact_sensitive(clean_note_raw).replace(['\r', '\n'], " ");
    let ts = diag::utc_rfc3339_now();
    let bullet = format!("- [{ts}] {clean_note}");

    let mem_path = ws_dir.join(MEMORY_FILE_REL);
    let existing = if mem_path.is_file() {
        fs::read_to_string(&mem_path)?
    } else {
        "# 宿主机长期维护记忆\n".to_string()
    };

    let target_heading = format!("## {eff_section}");
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    if let Some(hdr_idx) = lines.iter().position(|l| l.trim() == target_heading) {
        let mut insert_at = lines.len();
        for (idx, line) in lines.iter().enumerate().skip(hdr_idx + 1) {
            if line.starts_with("## ") {
                insert_at = idx;
                break;
            }
        }
        while insert_at > hdr_idx + 1 && lines[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        lines.insert(insert_at, bullet);
    } else {
        if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.push(target_heading);
        lines.push(String::new());
        lines.push(bullet);
    }

    let mut rendered = lines.join("\n");
    rendered.push('\n');
    sys::write_file_atomic(&mem_path, rendered.as_bytes(), Some(CRED_FILE_MODE))?;
    Ok(true)
}

fn check_core_runtime_perms(ws_dir: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let dir_checks = [
        (CREDENTIALS_DIR_REL, CRED_DIR_MODE),
        (CHECKPOINTS_DIR_REL, 0o700),
        (LOGS_DIR_REL, 0o700),
    ];
    for (rel, expected) in dir_checks {
        let p = ws_dir.join(rel);
        if p.is_dir() {
            if let Ok(meta) = fs::metadata(&p) {
                let actual = meta.permissions().mode() & 0o7777;
                if actual != expected {
                    errors.push(format!(
                        "{rel}: expected directory mode {expected:04o}, got {actual:04o}"
                    ));
                }
            }
        } else {
            errors.push(format!("{rel}: required directory missing"));
        }
    }

    let file_checks = [
        (AUDIT_LOG_REL, CRED_FILE_MODE),
        (MEMORY_FILE_REL, CRED_FILE_MODE),
        (WORKSPACE_MANIFEST_REL, CRED_FILE_MODE),
    ];
    for (rel, expected) in file_checks {
        let p = ws_dir.join(rel);
        if p.is_file() {
            if let Ok(meta) = fs::metadata(&p) {
                let actual = meta.permissions().mode() & 0o7777;
                if actual != expected {
                    errors.push(format!(
                        "{rel}: expected file mode {expected:04o}, got {actual:04o}"
                    ));
                }
            }
        } else {
            errors.push(format!("{rel}: required file missing"));
        }
    }

    let cp_dir = ws_dir.join(CHECKPOINTS_DIR_REL);
    if cp_dir.is_dir()
        && let Ok(read_dir) = fs::read_dir(&cp_dir)
    {
        for entry in read_dir.flatten() {
            let p = entry.path();
            if !p.is_dir() {
                continue;
            }
            if let Ok(meta) = fs::metadata(&p) {
                let actual = meta.permissions().mode() & 0o7777;
                if actual != 0o700 {
                    errors.push(format!(
                        "{}: expected checkpoint directory mode 0700, got {actual:04o}",
                        p.display()
                    ));
                }
            }
            for fname in ["manifest.json", "checkpoint.json"] {
                let fp = p.join(fname);
                if fp.is_file()
                    && let Ok(meta) = fs::metadata(&fp)
                {
                    let actual = meta.permissions().mode() & 0o7777;
                    if actual != CRED_FILE_MODE {
                        errors.push(format!(
                            "{}: expected file mode 0600, got {actual:04o}",
                            fp.display()
                        ));
                    }
                }
            }
        }
    }
    errors
}

fn inline_code_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"`[^`]*`").expect("valid inline code regex"))
}

fn open_outer_xml_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^<([a-zA-Z_][a-zA-Z0-9_-]*)(?:\s+[^>]*)?>$").expect("valid outer xml regex")
    })
}

fn inline_xml_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"</?([a-zA-Z_][a-zA-Z0-9_-]*)(?:\s+[^>]*)?>").expect("valid inline xml regex")
    })
}

fn xml_boundary_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?i)</?(?:guide|constraints_index|index|workflow|skill|card|task|protocol|zh_summary)\b",
        )
        .expect("valid xml boundary regex")
    })
}

fn table_sep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*\|[\s:-]+\|").expect("valid table sep regex"))
}

fn md_heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*#{1,6}\s+").expect("valid md heading regex"))
}

fn md_bullet_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^\s*[-*+]\s+").expect("valid md bullet regex"))
}

fn zh_summary_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)</?zh_summary\b").expect("valid zh_summary tag regex"))
}

fn section_hdr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[A-Z][A-Z0-9_]*:\s*$").expect("valid section hdr regex"))
}

fn kv_line_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^  +[A-Za-z0-9_.-]+:(?:\s+.*)?$").expect("valid kv line regex"))
}

fn banned_sep_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r" · |\]·\[|｜").expect("valid banned sep regex"))
}

fn md_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[[^\]]*\]\(([^)]+)\)").expect("valid md link regex"))
}

fn iso_date_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b").expect("valid iso date regex"))
}

fn strip_quotes(val: &str) -> String {
    let v = val.trim();
    if v.len() >= 2
        && ((v.starts_with('"') && v.ends_with('"')) || (v.starts_with('\'') && v.ends_with('\'')))
    {
        v[1..v.len() - 1].trim().to_string()
    } else {
        v.to_string()
    }
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(c) => out.push(c),
        }
    }
    out
}

fn resolve_index_item(item: &str, file_dir: &Path, ws_dir: &Path) -> PathBuf {
    if item.starts_with("./") || item.starts_with("../") {
        normalize_path(&file_dir.join(item))
    } else {
        normalize_path(&ws_dir.join(item))
    }
}

struct ParsedFrontmatter {
    strings: BTreeMap<String, String>,
    lists: BTreeMap<String, Vec<String>>,
    end_idx: usize,
}

fn parse_frontmatter(lines: &[&str]) -> std::result::Result<ParsedFrontmatter, String> {
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("line 1 must be '---'".to_string());
    }
    let Some(end_idx) = lines.iter().enumerate().skip(1).find_map(|(i, l)| {
        if l.trim() == "---" {
            Some(i)
        } else {
            None
        }
    }) else {
        return Err("unclosed YAML frontmatter (missing closing '---')".to_string());
    };

    let mut strings = BTreeMap::new();
    let mut lists: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut current_list_key: Option<String> = None;

    for raw in &lines[1..end_idx] {
        let stripped = raw.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if let Some(rest) = stripped.strip_prefix("- ")
            && let Some(ref key) = current_list_key
        {
            let item = strip_quotes(rest);
            if !item.is_empty() {
                lists.entry(key.clone()).or_default().push(item);
            }
            continue;
        }
        current_list_key = None;
        let Some((k_raw, v_raw)) = raw.split_once(':') else {
            continue;
        };
        let key = k_raw.trim().to_string();
        let val = v_raw.trim();
        if val.is_empty() {
            lists.entry(key.clone()).or_default();
            current_list_key = Some(key);
        } else if val.starts_with('[') && val.ends_with(']') {
            let inner = &val[1..val.len() - 1];
            let items: Vec<String> = inner
                .split(',')
                .map(strip_quotes)
                .filter(|s| !s.is_empty())
                .collect();
            lists.insert(key, items);
        } else {
            strings.insert(key, strip_quotes(val));
        }
    }

    Ok(ParsedFrontmatter {
        strings,
        lists,
        end_idx,
    })
}

fn check_agent_only_file(
    rel_path: &str,
    ws_dir: &Path,
) -> (Vec<String>, Option<Vec<String>>) {
    let mut errors = Vec::new();
    let abs_path = ws_dir.join(rel_path);
    let Ok(content) = fs::read_to_string(&abs_path) else {
        errors.push(format!("{rel_path}: required agent-only file missing"));
        return (errors, None);
    };
    let file_dir = abs_path.parent().unwrap_or(ws_dir);
    let lines: Vec<&str> = content.lines().collect();

    let fm = match parse_frontmatter(&lines) {
        Ok(fm) => fm,
        Err(msg) => {
            errors.push(format!("{rel_path}:1: {msg}"));
            return (errors, None);
        }
    };

    for key in ["name", "description", "zh_summary"] {
        if fm.strings.get(key).is_none_or(|v| v.trim().is_empty()) {
            errors.push(format!(
                "{rel_path}:1: frontmatter missing non-empty string '{key}'"
            ));
        }
    }
    if let Some(zh) = fm.strings.get("zh_summary")
        && !zh.trim().is_empty()
        && !has_cjk(zh)
    {
        errors.push(format!(
            "{rel_path}:1: frontmatter 'zh_summary' must contain CJK characters"
        ));
    }

    for key in ["tags", "index"] {
        if fm.lists.get(key).is_none_or(|v| v.is_empty()) {
            errors.push(format!(
                "{rel_path}:1: frontmatter '{key}' must be a non-empty list"
            ));
        }
    }

    let index_items = fm.lists.get("index").cloned();
    if let Some(ref items) = index_items {
        for item in items {
            if !item.contains('*') {
                let resolved = resolve_index_item(item, file_dir, ws_dir);
                if !resolved.exists() {
                    errors.push(format!(
                        "{rel_path}:1: frontmatter index path missing on disk: '{item}'"
                    ));
                }
            }
        }
    }

    let mut idx = fm.end_idx + 1;
    if idx >= lines.len() || !lines[idx].trim().is_empty() {
        errors.push(format!(
            "{rel_path}:{}: expected blank line after closing '---'",
            idx + 1
        ));
    }
    while idx < lines.len() && lines[idx].trim().is_empty() {
        idx += 1;
    }

    let mut outer_count = 0usize;
    let mut in_outer: Option<String> = None;
    let mut first_line_in_outer = false;

    for i in idx..lines.len() {
        let lineno = i + 1;
        let line = lines[i];
        let stripped = line.trim();
        let cleaned = inline_code_re().replace_all(line, "");

        if zh_summary_tag_re().is_match(&cleaned) {
            errors.push(format!("{rel_path}:{lineno}: <zh_summary> tag forbidden in body"));
        }
        if table_sep_re().is_match(line) || stripped.starts_with("---:") {
            errors.push(format!(
                "{rel_path}:{lineno}: Markdown table forbidden inside XML body"
            ));
        }

        if let Some(ref tag) = in_outer {
            if stripped == format!("</{tag}>") {
                in_outer = None;
                first_line_in_outer = false;
                continue;
            }
            if stripped.is_empty() {
                first_line_in_outer = false;
                continue;
            }
            for caps in inline_xml_re().captures_iter(&cleaned) {
                errors.push(format!(
                    "{rel_path}:{lineno}: nested XML tag <{}> inside <{tag}>",
                    &caps[1]
                ));
            }
            if md_heading_re().is_match(line) {
                errors.push(format!(
                    "{rel_path}:{lineno}: Markdown heading forbidden inside XML body"
                ));
            }
            if md_bullet_re().is_match(line) {
                errors.push(format!(
                    "{rel_path}:{lineno}: Markdown list bullet forbidden inside XML body"
                ));
            }
            if section_hdr_re().is_match(line) {
                if !first_line_in_outer && i > 0 && !lines[i - 1].trim().is_empty() {
                    errors.push(format!(
                        "{rel_path}:{lineno}: missing blank line before section '{stripped}'"
                    ));
                }
            } else if !kv_line_re().is_match(line) {
                errors.push(format!(
                    "{rel_path}:{lineno}: expected ALL-CAPS SECTION: or 2-space indented KEY: value line"
                ));
            }
            first_line_in_outer = false;
        } else {
            if stripped.is_empty() {
                continue;
            }
            if let Some(caps) = open_outer_xml_re().captures(stripped)
                && ALLOWED_XML_TAGS.contains(&&caps[1])
            {
                in_outer = Some(caps[1].to_string());
                outer_count += 1;
                first_line_in_outer = true;
            } else {
                errors.push(format!(
                    "{rel_path}:{lineno}: unexpected content outside outer XML tag"
                ));
            }
        }
    }

    if let Some(tag) = in_outer {
        errors.push(format!("{rel_path}: unclosed outer tag <{tag}>"));
    }
    if outer_count < 1 {
        errors.push(format!("{rel_path}: expected >= 1 outer XML block"));
    }

    (errors, index_items)
}

fn extract_relative_md_links(abs_path: &Path, ws_dir: &Path) -> BTreeSet<PathBuf> {
    let mut links = BTreeSet::new();
    let Ok(content) = fs::read_to_string(abs_path) else {
        return links;
    };
    let file_dir = abs_path.parent().unwrap_or(ws_dir);
    let mut in_fence = false;
    for line in content.lines() {
        let stripped = line.trim();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let cleaned = inline_code_re().replace_all(line, "");
        for caps in md_link_re().captures_iter(&cleaned) {
            let raw = caps[1].trim();
            let target = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '<' || c == '>');
            if target.is_empty()
                || target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with("file://")
                || target.starts_with('#')
            {
                continue;
            }
            let path_part = target
                .split('#')
                .next()
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            if path_part.is_empty() {
                continue;
            }
            let resolved = if let Some(stripped_abs) = path_part.strip_prefix('/') {
                normalize_path(&ws_dir.join(stripped_abs))
            } else {
                normalize_path(&file_dir.join(path_part))
            };
            links.insert(resolved);
        }
    }
    links
}

fn check_human_facing_file(rel_path: &str, ws_dir: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let abs_path = ws_dir.join(rel_path);
    let Ok(content) = fs::read_to_string(&abs_path) else {
        errors.push(format!("{rel_path}: required human-facing file missing"));
        return errors;
    };
    let file_dir = abs_path.parent().unwrap_or(ws_dir);
    let check_sep =
        rel_path.starts_with(".agents/constraints/") || rel_path.starts_with(".agents/memory/");

    let mut in_fence = false;
    for (idx, line) in content.lines().enumerate() {
        let lineno = idx + 1;
        let stripped = line.trim();
        if stripped.starts_with("```") || stripped.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let cleaned = inline_code_re().replace_all(line, "");
        if xml_boundary_re().is_match(&cleaned) {
            errors.push(format!(
                "{rel_path}:{lineno}: agent XML boundary tag forbidden in human-facing doc"
            ));
        }
        if check_sep && banned_sep_re().is_match(&cleaned) {
            errors.push(format!(
                "{rel_path}:{lineno}: banned separator ( · / ]·[ / ｜) forbidden"
            ));
        }
        for caps in md_link_re().captures_iter(&cleaned) {
            let raw = caps[1].trim();
            let target = raw
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(|c| c == '<' || c == '>');
            if target.is_empty()
                || target.starts_with("http://")
                || target.starts_with("https://")
                || target.starts_with("mailto:")
                || target.starts_with("file://")
                || target.starts_with('#')
            {
                continue;
            }
            let path_part = target
                .split('#')
                .next()
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("");
            if !path_part.ends_with(".md") {
                continue;
            }
            let resolved = if let Some(stripped_abs) = path_part.strip_prefix('/') {
                normalize_path(&ws_dir.join(stripped_abs))
            } else {
                normalize_path(&file_dir.join(path_part))
            };
            if !resolved.is_file() {
                errors.push(format!(
                    "{rel_path}:{lineno}: broken relative .md link '{target}'"
                ));
            }
        }
    }
    errors
}

/// Verify dual-audience format contracts (U30), date ban, bidirectional links, strict `0700`/`0600`
/// permissions (U29), and zero secret leaks outside `.agents/credentials/`.
pub fn verify_workspace(
    opts: &AgentWorkspaceOptions,
    runner: Option<&dyn CommandRunner>,
) -> Result<WorkspaceVerifyReport> {
    let ws_dir = opts.effective_workspace_dir();
    let mut checks_run = 0usize;
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // 1. Agent-only files (YAML frontmatter + single outer XML + section/KV rules).
    let mut index_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for &rel in AGENT_ONLY_FILES {
        checks_run += 1;
        let (file_errs, idx_items) = check_agent_only_file(rel, &ws_dir);
        errors.extend(file_errs);
        if let Some(items) = idx_items {
            index_map.insert(rel.to_string(), items);
        }
    }

    // 2. Human-facing files (0 XML tags, 0 banned separators, valid relative .md links).
    for &rel in HUMAN_FACING_FILES {
        checks_run += 1;
        errors.extend(check_human_facing_file(rel, &ws_dir));
    }

    // 3. Bidirectional README.md <-> INDEX.md links where both exist.
    for &rel_idx in AGENT_ONLY_FILES {
        if !rel_idx.ends_with("INDEX.md") {
            continue;
        }
        let idx_abs = normalize_path(&ws_dir.join(rel_idx));
        let Some(dir) = idx_abs.parent() else {
            continue;
        };
        let readme_abs = normalize_path(&dir.join("README.md"));
        if !readme_abs.is_file() || !idx_abs.is_file() {
            continue;
        }
        checks_run += 1;
        let readme_links = extract_relative_md_links(&readme_abs, &ws_dir);
        if !readme_links.contains(&idx_abs) {
            errors.push(format!(
                "{}: missing bidirectional link to sibling '{rel_idx}'",
                readme_abs.display()
            ));
        }
        let idx_targets: BTreeSet<PathBuf> = index_map
            .get(rel_idx)
            .into_iter()
            .flatten()
            .filter(|item| !item.contains('*'))
            .map(|item| resolve_index_item(item, dir, &ws_dir))
            .collect();
        if !idx_targets.contains(&readme_abs) {
            errors.push(format!(
                "{rel_idx}: frontmatter 'index' missing bidirectional reference to sibling README.md"
            ));
        }
    }

    // 4. Zero YYYY-MM-DD dates in .agents/CONSTRAINTS.md and .agents/constraints/*.md.
    for &rel in DATE_BAN_FILES {
        checks_run += 1;
        let abs_path = ws_dir.join(rel);
        if let Ok(content) = fs::read_to_string(&abs_path) {
            for (idx, line) in content.lines().enumerate() {
                if let Some(m) = iso_date_re().find(line) {
                    errors.push(format!(
                        "{rel}:{}: date '{}' forbidden in constraint file",
                        idx + 1,
                        m.as_str()
                    ));
                }
            }
        }
    }

    // 5. Strict 0700/0600 permissions on runtime directories & files.
    checks_run += 1;
    errors.extend(check_core_runtime_perms(&ws_dir));

    // 6. U29 credential audit + workspace-wide secret leak scan outside .agents/credentials/.
    checks_run += 1;
    let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
    let cred_report = credentials::audit_credentials(&cred_dir, std::slice::from_ref(&ws_dir), runner);
    for finding in cred_report.findings {
        let formatted = format!("[{}] {}: {}", finding.code, finding.path, finding.message);
        if finding.severity == "error" {
            errors.push(formatted);
        } else {
            warnings.push(formatted);
        }
    }

    Ok(WorkspaceVerifyReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        passed: errors.is_empty(),
        checks_run,
        errors,
        warnings,
    })
}
