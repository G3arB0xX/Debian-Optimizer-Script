pub mod adapters;
pub mod memory;
pub mod migrate;

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

pub use crate::error::{
    EXIT_AUDIT_NONSTANDARD, EXIT_MEMORY_BLOAT, EXIT_MEMORY_SECRET_LEAK, EXIT_MIGRATION_CONFLICT,
    EXIT_SECRET_DETECTED, EXIT_WRITE_GATE_REJECT,
};
pub use adapters::{
    AdapterRegistry, HarnessAdapterSpec, ManagedProjection, OverrideAction, OverrideReport,
    RepoBlueprint, SwitchMode, SwitchReport, builtin_adapter_registry, load_adapter_registry,
    load_repo_blueprint, manage_override, switch_harnesses,
};
pub use memory::{
    ConsolidatePlanItem, ConsolidateReport, DEFAULT_SHARD_SUBDIR_THRESHOLD,
    MAX_SEARCH_SNIPPET_LINES, MEMORY_ARCHIVE_DIR_REL, MEMORY_DIR_REL, MEMORY_INDEX_MD_REL,
    MemoryIndexReport, MemoryLintReport, MemorySearchHit, MemorySearchReport, MemoryTopicInput,
    MemoryTopicMeta, MemoryTracking, PromoteReport, RAW_FENCE_MAX_LINES, TOC_MAX_BYTES,
    TOC_MAX_LINES, TOPIC_MAX_BYTES, TOPIC_MAX_LINES, TopicStatus, TopicType, WriteGateReport,
    WriteGateVerdict, append_memory_note, consolidate_memory, lint_memory, promote_memory,
    rebuild_memory_index, search_memory, write_memory_topic,
};
pub use migrate::{
    AuditFinding, IdeCacheAuditOptions, LegacyItemKind, MigrateReport, WorkspaceAuditReport,
    audit_workspace, audit_workspace_with_options, migrate_workspace,
};

pub const DEFAULT_WORKSPACE_DIR: &str = "/var/lib/debopty/agent";
pub const WORKSPACE_SCHEMA_VERSION: &str = "2";
pub const WIRE_EPOCH: &str = "auto-swarm/v1";
pub const AUDIT_LOG_REL: &str = ".agents/logs/audit.jsonl";
pub const CHECKPOINTS_DIR_REL: &str = ".agents/checkpoints";
pub const CREDENTIALS_DIR_REL: &str = ".agents/credentials";
pub const MEMORY_FILE_REL: &str = ".agents/MEMORY.md";
pub const LEGACY_ROOT_MEMORY_REL: &str = "MEMORY.md";
pub const HOST_PROFILE_REL: &str = ".agents/memory/host-profile.md";
pub const LEGACY_SSOT_REL: &str = "docs/agent/constraints/90_project_legacy_ssot.md";
pub const OLD_LEGACY_SSOT_REL: &str = ".agents/constraints/90_project_legacy_ssot.md";
pub const DECISIONS_FILE_REL: &str = "docs/agent/decisions.md";
pub const SCHEMAS_DIR_REL: &str = ".agents/schemas";
pub const OVERRIDES_DIR_REL: &str = ".agents/overrides";
pub const CACHE_DIR_REL: &str = ".agents/cache";
pub const TMP_DIR_REL: &str = ".agents/tmp";
pub const SKILLS_DIR_REL: &str = ".agents/skills";
pub const RULES_DIR_REL: &str = ".agents/rules";
pub const REPO_BLUEPRINT_REL: &str = ".agents/schemas/repo_blueprint.json";
pub const HARNESS_ADAPTERS_REL: &str = ".agents/schemas/harness_adapters.json";
pub const MCP_SERVERS_REL: &str = ".agents/schemas/mcp_servers.json";
pub const MEMORY_INDEX_CACHE_REL: &str = ".agents/cache/memory_index.json";
pub const MANAGED_START_MARKER: &str = "<!-- managed-by-debopty:start -->";
pub const MANAGED_END_MARKER: &str = "<!-- managed-by-debopty:end -->";
pub const GITIGNORE_START_MARKER: &str = "# managed-by-debopty:start";
pub const GITIGNORE_END_MARKER: &str = "# managed-by-debopty:end";

const DIAG_TARGET: &str = "debopty::agent_workspace";
const WORKSPACE_MANIFEST_REL: &str = ".agents/workspace.json";
const LOGS_DIR_REL: &str = ".agents/logs";
const DEFAULT_DEBOPTY_BIN: &str = "/usr/local/bin/debopty";

const MANAGED_DOC_TEMPLATES: &[(&str, &str, u32)] = &[
    ("templates/agent/README.md.tmpl", "README.md", 0o644),
    ("templates/agent/INDEX.md.tmpl", ".agents/INDEX.md", 0o644),
    (
        "templates/agent/DOT_AGENTS_README.md.tmpl",
        ".agents/README.md",
        0o644,
    ),
    (
        "templates/agent/docs_agent/INDEX.md.tmpl",
        "docs/agent/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/docs_agent/README.md.tmpl",
        "docs/agent/README.md",
        0o644,
    ),
    (
        "templates/agent/CONSTRAINTS.md.tmpl",
        "docs/agent/CONSTRAINTS.md",
        0o644,
    ),
    (
        "templates/agent/constraints/INDEX.md.tmpl",
        "docs/agent/constraints/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/constraints/README.md.tmpl",
        "docs/agent/constraints/README.md",
        0o644,
    ),
    (
        "templates/agent/constraints/00_vps_general_and_docs.md.tmpl",
        "docs/agent/constraints/00_vps_general_and_docs.md",
        0o644,
    ),
    (
        "templates/agent/constraints/10_system_change_and_style.md.tmpl",
        "docs/agent/constraints/10_system_change_and_style.md",
        0o644,
    ),
    (
        "templates/agent/constraints/20_security_and_credentials.md.tmpl",
        "docs/agent/constraints/20_security_and_credentials.md",
        0o644,
    ),
    (
        "templates/agent/constraints/30_atomic_ops_and_audit.md.tmpl",
        "docs/agent/constraints/30_atomic_ops_and_audit.md",
        0o644,
    ),
    (
        "templates/agent/constraints/40_memory_and_retention.md.tmpl",
        "docs/agent/constraints/40_memory_and_retention.md",
        0o644,
    ),
    (
        "templates/agent/constraints/50_harness_decoupling_and_migration.md.tmpl",
        "docs/agent/constraints/50_harness_decoupling_and_migration.md",
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
    (
        "templates/agent/schemas/INDEX.md.tmpl",
        ".agents/schemas/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/schemas/README.md.tmpl",
        ".agents/schemas/README.md",
        0o644,
    ),
    (
        "templates/agent/schemas/dot_agent_spec.json.tmpl",
        ".agents/schemas/dot_agent_spec.json",
        0o644,
    ),
    (
        "templates/agent/schemas/memory_topic.schema.json.tmpl",
        ".agents/schemas/memory_topic.schema.json",
        0o644,
    ),
    (
        "templates/agent/schemas/workspace_manifest.schema.json.tmpl",
        ".agents/schemas/workspace_manifest.schema.json",
        0o644,
    ),
    (
        "templates/agent/schemas/harness_adapters.json.tmpl",
        ".agents/schemas/harness_adapters.json",
        0o644,
    ),
    (
        "templates/agent/overrides/INDEX.md.tmpl",
        ".agents/overrides/INDEX.md",
        0o644,
    ),
    (
        "templates/agent/overrides/README.md.tmpl",
        ".agents/overrides/README.md",
        0o644,
    ),
    (
        "templates/agent/overrides/_template.md.tmpl",
        ".agents/overrides/_template.md",
        0o644,
    ),
];

const AGENTS_ENTRYPOINT_TEMPLATE: &str = "templates/agent/AGENTS.md.tmpl";
const DECISIONS_SEED_TEMPLATE: &str = "templates/agent/docs_agent/decisions.md.tmpl";
const HOST_PROFILE_TEMPLATE: &str = "templates/agent/memory/host-profile.md.tmpl";
const MEMORY_SEED_TEMPLATE: &str = "templates/agent/MEMORY.md.tmpl";
const GITIGNORE_TEMPLATE: &str = "templates/agent/gitignore.tmpl";
const REPO_BLUEPRINT_TEMPLATE: &str = "templates/agent/schemas/repo_blueprint.json.tmpl";

const AGENT_ONLY_FILES: &[&str] = &[
    "AGENTS.md",
    ".agents/INDEX.md",
    ".agents/memory/INDEX.md",
    ".agents/schemas/INDEX.md",
    ".agents/overrides/INDEX.md",
    "docs/agent/INDEX.md",
    "docs/agent/CONSTRAINTS.md",
    "docs/agent/constraints/INDEX.md",
];

const HUMAN_FACING_FILES: &[&str] = &[
    "README.md",
    ".agents/README.md",
    ".agents/MEMORY.md",
    ".agents/memory/README.md",
    ".agents/memory/host-profile.md",
    ".agents/schemas/README.md",
    ".agents/overrides/README.md",
    ".agents/overrides/_template.md",
    "docs/agent/README.md",
    "docs/agent/decisions.md",
    "docs/agent/constraints/README.md",
    "docs/agent/constraints/00_vps_general_and_docs.md",
    "docs/agent/constraints/10_system_change_and_style.md",
    "docs/agent/constraints/20_security_and_credentials.md",
    "docs/agent/constraints/30_atomic_ops_and_audit.md",
    "docs/agent/constraints/40_memory_and_retention.md",
    "docs/agent/constraints/50_harness_decoupling_and_migration.md",
];

const DATE_BAN_FILES: &[&str] = &[
    "docs/agent/CONSTRAINTS.md",
    "docs/agent/constraints/INDEX.md",
    "docs/agent/constraints/README.md",
    "docs/agent/constraints/00_vps_general_and_docs.md",
    "docs/agent/constraints/10_system_change_and_style.md",
    "docs/agent/constraints/20_security_and_credentials.md",
    "docs/agent/constraints/30_atomic_ops_and_audit.md",
    "docs/agent/constraints/40_memory_and_retention.md",
    "docs/agent/constraints/50_harness_decoupling_and_migration.md",
];

const VENDOR_NEUTRAL_FILES: &[&str] = &[
    "AGENTS.md",
    "README.md",
    ".agents/INDEX.md",
    ".agents/README.md",
    ".agents/memory/INDEX.md",
    ".agents/memory/README.md",
    ".agents/schemas/INDEX.md",
    ".agents/schemas/README.md",
    "docs/agent/INDEX.md",
    "docs/agent/README.md",
    "docs/agent/CONSTRAINTS.md",
    "docs/agent/constraints/INDEX.md",
    "docs/agent/constraints/README.md",
    "docs/agent/constraints/00_vps_general_and_docs.md",
    "docs/agent/constraints/10_system_change_and_style.md",
    "docs/agent/constraints/20_security_and_credentials.md",
    "docs/agent/constraints/30_atomic_ops_and_audit.md",
    "docs/agent/constraints/40_memory_and_retention.md",
    "docs/agent/constraints/50_harness_decoupling_and_migration.md",
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
    pub harnesses: Option<String>,
    pub memory_tracking: Option<MemoryTracking>,
    pub migrate_legacy: bool,
    pub cleanup_legacy: bool,
    pub adapter_file: Option<PathBuf>,
}

impl AgentWorkspaceOptions {
    pub fn new(root_prefix: impl Into<PathBuf>, workspace_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_prefix: root_prefix.into(),
            workspace_dir: workspace_dir.into(),
            template_dir: None,
            force_overwrite_managed: false,
            harnesses: None,
            memory_tracking: None,
            migrate_legacy: false,
            cleanup_legacy: false,
            adapter_file: None,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationState {
    #[serde(default)]
    pub migrated_from_v1: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_migrated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checkpoint_id: Option<String>,
    #[serde(default)]
    pub migrated_source_hashes: Vec<String>,
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
    pub memory_tracking: MemoryTracking,
    pub memory_tracking_notice: String,
    pub active_harnesses: Vec<String>,
    pub migrated_legacy_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
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
    pub wire_epoch: String,
    pub memory_tracking: MemoryTracking,
    pub active_harnesses: Vec<String>,
    pub managed_files_present: usize,
    pub managed_files_expected: usize,
    pub missing_files: Vec<String>,
    pub memory_exists: bool,
    pub memory_lines: usize,
    pub memory_topic_count: usize,
    pub override_count: usize,
    pub legacy_detected: bool,
    pub blueprint_valid: bool,
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

fn default_wire_epoch() -> String {
    WIRE_EPOCH.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkspaceManifest {
    pub schema_version: String,
    pub debopty_version: String,
    #[serde(default = "default_wire_epoch")]
    pub wire_epoch: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub memory_tracking: MemoryTracking,
    #[serde(default)]
    pub active_harnesses: Vec<String>,
    #[serde(default)]
    pub managed_hashes: BTreeMap<String, String>,
    #[serde(default)]
    pub migration_state: MigrationState,
}

pub(crate) fn current_debopty_version() -> String {
    let id = version::compiled_identity();
    id.primary()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "0.0.0".to_string())
}

pub(crate) fn memory_tracking_notice(tracking: MemoryTracking) -> String {
    match tracking {
        MemoryTracking::Local => {
            "记忆追踪策略：local（默认，`.agents/MEMORY.md` 与 `.agents/memory/` 已写入 `.gitignore` 仅本地保留；如需 Git 共享可传入 `--memory-tracking tracked`）".to_string()
        }
        MemoryTracking::Tracked => {
            "记忆追踪策略：tracked（`.agents/MEMORY.md` 与 `.agents/memory/` 允许 Git 追踪，`host-profile.md` 及运行态目录仍保持忽略；请严格遵守 Write Gate 零明文密钥约束）".to_string()
        }
    }
}

pub(crate) fn ensure_dir_mode(path: &Path, mode: u32) -> Result<bool> {
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

pub(crate) fn load_manifest(ws_dir: &Path) -> Option<WorkspaceManifest> {
    fs::read_to_string(ws_dir.join(WORKSPACE_MANIFEST_REL))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(crate) fn save_manifest(ws_dir: &Path, manifest: &WorkspaceManifest) -> Result<()> {
    let mut json = serde_json::to_string_pretty(manifest)
        .map_err(|e| CoreError::System(format!("serialize workspace manifest failed: {e}")))?;
    json.push('\n');
    sys::write_file_atomic(
        &ws_dir.join(WORKSPACE_MANIFEST_REL),
        json.as_bytes(),
        Some(CRED_FILE_MODE),
    )?;
    Ok(())
}

pub(crate) fn ws_rel_to_checkpoint_path(opts: &AgentWorkspaceOptions, rel: &str) -> String {
    let clean_rel = rel.trim_start_matches('/');
    if opts.root_prefix.as_os_str().is_empty() || opts.root_prefix == Path::new("/") {
        return opts
            .effective_workspace_dir()
            .join(clean_rel)
            .to_string_lossy()
            .into_owned();
    }
    if opts.workspace_dir.starts_with(&opts.root_prefix)
        && let Ok(stripped) = opts.workspace_dir.strip_prefix(&opts.root_prefix)
    {
        return format!(
            "/{}/{}",
            stripped.to_string_lossy().trim_matches('/'),
            clean_rel
        );
    }
    format!(
        "/{}/{}",
        opts.workspace_dir.to_string_lossy().trim_matches('/'),
        clean_rel
    )
}

pub(crate) fn build_template_vars(
    opts: &AgentWorkspaceOptions,
    ws_dir: &Path,
    debopty_version: &str,
    now: &str,
    memory_tracking: MemoryTracking,
    active_harnesses: &[String],
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
    vars.insert("WIRE_EPOCH".to_string(), WIRE_EPOCH.to_string());
    vars.insert(
        "MEMORY_TRACKING".to_string(),
        memory_tracking.as_str().to_string(),
    );
    let gitignore_rules = match memory_tracking {
        MemoryTracking::Local => ".agents/MEMORY.md\n.agents/memory/",
        MemoryTracking::Tracked => ".agents/memory/host-profile.md",
    };
    vars.insert(
        "MEMORY_TRACKING_GITIGNORE_RULES".to_string(),
        gitignore_rules.to_string(),
    );
    let harnesses_json =
        serde_json::to_string(active_harnesses).unwrap_or_else(|_| "[]".to_string());
    vars.insert("ACTIVE_HARNESSES_JSON".to_string(), harnesses_json);
    vars
}

pub(crate) fn extract_managed_block_any(text: &str) -> Option<(usize, usize, &str)> {
    const PAIRS: &[(&str, &str)] = &[
        (MANAGED_START_MARKER, MANAGED_END_MARKER),
        (
            "<!-- managed-by-mem_ctl:start -->",
            "<!-- managed-by-mem_ctl:end -->",
        ),
        (
            "<!-- managed-by-repo_ctl:start -->",
            "<!-- managed-by-repo_ctl:end -->",
        ),
    ];
    for &(start_m, end_m) in PAIRS {
        if let Some(start) = text.find(start_m) {
            let after_start = start + start_m.len();
            if let Some(end_rel) = text[after_start..].find(end_m) {
                let end = after_start + end_rel + end_m.len();
                return Some((start, end, &text[start..end]));
            }
        }
    }
    None
}

pub(crate) fn extract_gitignore_block_any(text: &str) -> Option<(usize, usize, &str)> {
    const PAIRS: &[(&str, &str)] = &[
        (GITIGNORE_START_MARKER, GITIGNORE_END_MARKER),
        ("# managed-by-repo_ctl:start", "# managed-by-repo_ctl:end"),
    ];
    for &(start_m, end_m) in PAIRS {
        if let Some(start) = text.find(start_m) {
            let after_start = start + start_m.len();
            if let Some(end_rel) = text[after_start..].find(end_m) {
                let end = after_start + end_rel + end_m.len();
                return Some((start, end, &text[start..end]));
            }
        }
    }
    None
}

fn normalize_managed_block_for_compare(block: &str) -> String {
    block
        .lines()
        .filter(|line| !line.trim_start().starts_with("- 最近刷新时间："))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn validate_safe_identifier(raw: &str, kind: &str) -> Result<()> {
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

/// Idempotently initialize or non-destructively upgrade the standardized VPS agent workspace (schema v2).
pub fn init_workspace(opts: &AgentWorkspaceOptions) -> Result<InitWorkspaceReport> {
    init_workspace_core(opts, true)
}

pub(crate) fn init_workspace_core(
    opts: &AgentWorkspaceOptions,
    allow_auto_migrate: bool,
) -> Result<InitWorkspaceReport> {
    let ws_dir = opts.effective_workspace_dir();
    let debopty_version = current_debopty_version();
    let now = diag::utc_rfc3339_now();
    let tpl = TemplateEngine::new(opts.template_dir.clone()).with_root_prefix(&opts.root_prefix);

    let existing_manifest = load_manifest(&ws_dir);
    let effective_tracking = opts
        .memory_tracking
        .or_else(|| existing_manifest.as_ref().map(|m| m.memory_tracking))
        .unwrap_or_default();
    let existing_harnesses = existing_manifest
        .as_ref()
        .map(|m| m.active_harnesses.clone())
        .unwrap_or_default();
    let vars = build_template_vars(
        opts,
        &ws_dir,
        &debopty_version,
        &now,
        effective_tracking,
        &existing_harnesses,
    );

    let mut changed = false;
    // 0755 canonical directories
    for rel_dir in [
        "",
        ".agents",
        SCHEMAS_DIR_REL,
        OVERRIDES_DIR_REL,
        SKILLS_DIR_REL,
        RULES_DIR_REL,
        "docs",
        "docs/agent",
        "docs/agent/constraints",
    ] {
        let dir_path = if rel_dir.is_empty() {
            ws_dir.clone()
        } else {
            ws_dir.join(rel_dir)
        };
        changed |= ensure_dir_mode(&dir_path, 0o755)?;
    }
    // 0700 runtime/sensitive directories
    for rel_dir in [
        MEMORY_DIR_REL,
        MEMORY_ARCHIVE_DIR_REL,
        LOGS_DIR_REL,
        CHECKPOINTS_DIR_REL,
        CREDENTIALS_DIR_REL,
        CACHE_DIR_REL,
        TMP_DIR_REL,
    ] {
        let mode = if rel_dir == CREDENTIALS_DIR_REL {
            CRED_DIR_MODE
        } else {
            0o700
        };
        changed |= ensure_dir_mode(&ws_dir.join(rel_dir), mode)?;
    }

    let mut managed_hashes = existing_manifest
        .as_ref()
        .map(|m| m.managed_hashes.clone())
        .unwrap_or_default();

    let mut created_files = Vec::new();
    let mut updated_files = Vec::new();
    let mut preserved_files = Vec::new();
    let mut unchanged_files = Vec::new();

    // 0. Root `AGENTS.md` (managed marker block inside `<guide>`, preserving user text outside markers;
    // falls back to 3-way SHA-256 upgrade if upgrading from an older template without markers).
    {
        let rendered = tpl.render_to_string(AGENTS_ENTRYPOINT_TEMPLATE, &vars)?;
        let new_hash = sha256_hex(rendered.as_bytes());
        let dest = ws_dir.join("AGENTS.md");
        if !dest.is_file() {
            sys::write_file_atomic(&dest, rendered.as_bytes(), Some(0o644))?;
            managed_hashes.insert("AGENTS.md".to_string(), new_hash);
            created_files.push("AGENTS.md".to_string());
            changed = true;
        } else {
            let disk_bytes = fs::read(&dest)?;
            let disk_hash = sha256_hex(&disk_bytes);
            let disk_text = String::from_utf8_lossy(&disk_bytes);
            if disk_hash == new_hash {
                let cur_mode = fs::metadata(&dest)?.permissions().mode() & 0o7777;
                if cur_mode != 0o644 {
                    fs::set_permissions(&dest, fs::Permissions::from_mode(0o644))?;
                    changed = true;
                }
                managed_hashes.insert("AGENTS.md".to_string(), new_hash);
                unchanged_files.push("AGENTS.md".to_string());
            } else if let (Some((d_start, d_end, d_block)), Some((_, _, n_block))) = (
                extract_managed_block_any(&disk_text),
                extract_managed_block_any(&rendered),
            ) {
                if d_block.trim() == n_block.trim() {
                    managed_hashes.insert("AGENTS.md".to_string(), disk_hash);
                    unchanged_files.push("AGENTS.md".to_string());
                } else {
                    let mut spliced = String::with_capacity(disk_text.len() + n_block.len());
                    spliced.push_str(&disk_text[..d_start]);
                    spliced.push_str(n_block);
                    spliced.push_str(&disk_text[d_end..]);
                    let spliced_hash = sha256_hex(spliced.as_bytes());
                    sys::write_file_atomic(&dest, spliced.as_bytes(), Some(0o644))?;
                    managed_hashes.insert("AGENTS.md".to_string(), spliced_hash);
                    updated_files.push("AGENTS.md".to_string());
                    changed = true;
                }
            } else {
                let prev_matches = managed_hashes
                    .get("AGENTS.md")
                    .is_some_and(|prev| prev == &disk_hash);
                if opts.force_overwrite_managed || prev_matches {
                    sys::write_file_atomic(&dest, rendered.as_bytes(), Some(0o644))?;
                    managed_hashes.insert("AGENTS.md".to_string(), new_hash);
                    updated_files.push("AGENTS.md".to_string());
                    changed = true;
                } else {
                    preserved_files.push("AGENTS.md".to_string());
                }
            }
        }
    }

    // 1. 3-way SHA-256 managed documentation & schema templates.
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

    // 2. Managed repo_blueprint.json (preserves managed_projections across re-init)
    {
        let bp_path = ws_dir.join(REPO_BLUEPRINT_REL);
        if !bp_path.is_file() {
            let rendered = tpl.render_to_string(REPO_BLUEPRINT_TEMPLATE, &vars)?;
            sys::write_file_atomic(&bp_path, rendered.as_bytes(), Some(0o644))?;
            created_files.push(REPO_BLUEPRINT_REL.to_string());
            changed = true;
        } else if let Some(mut bp) = load_repo_blueprint(&ws_dir) {
            if bp.memory_tracking != effective_tracking
                || bp.debopty_version != debopty_version
                || bp.blueprint_schema_version < 2
            {
                bp.blueprint_schema_version = 2;
                bp.memory_tracking = effective_tracking;
                bp.debopty_version = debopty_version.clone();
                bp.updated_at = now.clone();
                adapters::save_repo_blueprint(&ws_dir, &bp)?;
                updated_files.push(REPO_BLUEPRINT_REL.to_string());
                changed = true;
            } else {
                unchanged_files.push(REPO_BLUEPRINT_REL.to_string());
            }
        }
    }

    // 3. Managed `.gitignore` block per memory_tracking (preserving user lines outside markers)
    {
        let rendered = tpl.render_to_string(GITIGNORE_TEMPLATE, &vars)?;
        let block = rendered.trim_end();
        let gi_path = ws_dir.join(".gitignore");
        if !gi_path.is_file() {
            let mut out = block.to_string();
            out.push('\n');
            sys::write_file_atomic(&gi_path, out.as_bytes(), Some(0o644))?;
            created_files.push(".gitignore".to_string());
            changed = true;
        } else {
            let disk_text = fs::read_to_string(&gi_path)?;
            if let Some((d_start, d_end, d_block)) = extract_gitignore_block_any(&disk_text) {
                if d_block.trim() == block.trim() {
                    unchanged_files.push(".gitignore".to_string());
                } else {
                    let mut spliced = String::with_capacity(disk_text.len() + block.len());
                    spliced.push_str(&disk_text[..d_start]);
                    spliced.push_str(block);
                    spliced.push_str(&disk_text[d_end..]);
                    if !spliced.ends_with('\n') {
                        spliced.push('\n');
                    }
                    sys::write_file_atomic(&gi_path, spliced.as_bytes(), Some(0o644))?;
                    updated_files.push(".gitignore".to_string());
                    changed = true;
                }
            } else {
                let mut appended = disk_text.trim_end().to_string();
                if !appended.is_empty() {
                    appended.push_str("\n\n");
                }
                appended.push_str(block);
                appended.push('\n');
                sys::write_file_atomic(&gi_path, appended.as_bytes(), Some(0o644))?;
                updated_files.push(".gitignore".to_string());
                changed = true;
            }
        }
    }

    // 4. Managed marker block file: `.agents/memory/host-profile.md` (`0600`, custom tail preserved).
    {
        let rendered = tpl.render_to_string(HOST_PROFILE_TEMPLATE, &vars)?;
        let dest = ws_dir.join(HOST_PROFILE_REL);
        if !dest.is_file() {
            sys::write_file_atomic(&dest, rendered.as_bytes(), Some(CRED_FILE_MODE))?;
            created_files.push(HOST_PROFILE_REL.to_string());
            changed = true;
        } else {
            let disk_text = fs::read_to_string(&dest)?;
            let cur_mode = fs::metadata(&dest)?.permissions().mode() & 0o7777;
            match (
                extract_managed_block_any(&disk_text),
                extract_managed_block_any(&rendered),
            ) {
                (Some((d_start, d_end, d_block)), Some((_, _, n_block))) => {
                    if normalize_managed_block_for_compare(d_block)
                        == normalize_managed_block_for_compare(n_block)
                    {
                        if cur_mode != CRED_FILE_MODE {
                            fs::set_permissions(&dest, fs::Permissions::from_mode(CRED_FILE_MODE))?;
                            changed = true;
                        }
                        unchanged_files.push(HOST_PROFILE_REL.to_string());
                    } else {
                        let mut spliced = String::with_capacity(disk_text.len() + n_block.len());
                        spliced.push_str(&disk_text[..d_start]);
                        spliced.push_str(n_block);
                        spliced.push_str(&disk_text[d_end..]);
                        sys::write_file_atomic(&dest, spliced.as_bytes(), Some(CRED_FILE_MODE))?;
                        updated_files.push(HOST_PROFILE_REL.to_string());
                        changed = true;
                    }
                }
                _ => {
                    if opts.force_overwrite_managed {
                        sys::write_file_atomic(&dest, rendered.as_bytes(), Some(CRED_FILE_MODE))?;
                        updated_files.push(HOST_PROFILE_REL.to_string());
                        changed = true;
                    } else {
                        if cur_mode != CRED_FILE_MODE {
                            fs::set_permissions(&dest, fs::Permissions::from_mode(CRED_FILE_MODE))?;
                            changed = true;
                        }
                        preserved_files.push(HOST_PROFILE_REL.to_string());
                    }
                }
            }
        }
    }

    // 5. Seed-only `.agents/MEMORY.md` (`0600`, NEVER overwritten once present).
    let had_canonical_memory = ws_dir.join(MEMORY_FILE_REL).is_file();
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

    // 5b. Seed `docs/agent/decisions.md` (`0644`, created if absent, existing ADR entries always preserved).
    {
        let decisions_path = ws_dir.join(DECISIONS_FILE_REL);
        if !decisions_path.is_file() {
            let rendered = tpl.render_to_string(DECISIONS_SEED_TEMPLATE, &vars)?;
            sys::write_file_atomic(&decisions_path, rendered.as_bytes(), Some(0o644))?;
            created_files.push(DECISIONS_FILE_REL.to_string());
            changed = true;
        } else {
            let cur_mode = fs::metadata(&decisions_path)?.permissions().mode() & 0o7777;
            if cur_mode != 0o644 {
                fs::set_permissions(&decisions_path, fs::Permissions::from_mode(0o644))?;
                changed = true;
            }
            preserved_files.push(DECISIONS_FILE_REL.to_string());
        }
    }

    // 5c. Upgrade legacy `.agents/CONSTRAINTS.md` & `.agents/constraints/` to `docs/agent/` layout:
    // preserve `90_project_legacy_ssot.md` if present, and remove only unmodified old managed files.
    {
        let old_ssot = ws_dir.join(OLD_LEGACY_SSOT_REL);
        let new_ssot = ws_dir.join(LEGACY_SSOT_REL);
        if old_ssot.is_file() && !new_ssot.is_file() {
            let bytes = fs::read(&old_ssot)?;
            sys::write_file_atomic(&new_ssot, &bytes, Some(0o644))?;
            let _ = fs::remove_file(&old_ssot);
            updated_files.push(LEGACY_SSOT_REL.to_string());
            changed = true;
        }
        let legacy_managed_paths = [
            ".agents/CONSTRAINTS.md",
            ".agents/constraints/INDEX.md",
            ".agents/constraints/README.md",
            ".agents/constraints/00_vps_general_and_docs.md",
            ".agents/constraints/10_system_change_and_style.md",
            ".agents/constraints/20_security_and_credentials.md",
            ".agents/constraints/30_atomic_ops_and_audit.md",
            ".agents/constraints/40_memory_and_retention.md",
            ".agents/constraints/50_harness_decoupling_and_migration.md",
        ];
        for old_rel in legacy_managed_paths {
            let old_abs = ws_dir.join(old_rel);
            if old_abs.is_file()
                && let Ok(bytes) = fs::read(&old_abs)
                && managed_hashes
                    .get(old_rel)
                    .is_some_and(|h| h == &sha256_hex(&bytes))
            {
                let _ = fs::remove_file(&old_abs);
                managed_hashes.remove(old_rel);
                changed = true;
            }
        }
        let old_constraints_dir = ws_dir.join(".agents/constraints");
        if old_constraints_dir.is_dir()
            && fs::read_dir(&old_constraints_dir).is_ok_and(|mut it| it.next().is_none())
        {
            let _ = fs::remove_dir(&old_constraints_dir);
        }
    }

    // 6. Append-only audit.jsonl (`0600`, created empty if absent, NEVER truncated).
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

    // 7. Persist workspace.json (`0600`) only when created or changed (keeps re-init 100% idempotent).
    let manifest_path = ws_dir.join(WORKSPACE_MANIFEST_REL);
    let manifest_needs_write = changed
        || existing_manifest.as_ref().is_none_or(|m| {
            m.schema_version != WORKSPACE_SCHEMA_VERSION
                || m.debopty_version != debopty_version
                || m.memory_tracking != effective_tracking
                || m.managed_hashes != managed_hashes
        });
    if manifest_needs_write {
        let created_at = existing_manifest
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| now.clone());
        let migration_state = existing_manifest
            .as_ref()
            .map(|m| m.migration_state.clone())
            .unwrap_or_default();
        let manifest = WorkspaceManifest {
            schema_version: WORKSPACE_SCHEMA_VERSION.to_string(),
            debopty_version: debopty_version.clone(),
            wire_epoch: WIRE_EPOCH.to_string(),
            created_at,
            updated_at: now.clone(),
            memory_tracking: effective_tracking,
            active_harnesses: existing_harnesses.clone(),
            managed_hashes,
            migration_state,
        };
        save_manifest(&ws_dir, &manifest)?;
        changed = true;
    } else {
        let cur_mode = fs::metadata(&manifest_path)?.permissions().mode() & 0o7777;
        if cur_mode != CRED_FILE_MODE {
            fs::set_permissions(&manifest_path, fs::Permissions::from_mode(CRED_FILE_MODE))?;
            changed = true;
        }
    }

    // 8. Automatic v1->v2 seamless lossless migration if legacy root MEMORY.md exists or migrate_legacy set
    let mut migrated_legacy_count = 0usize;
    let mut checkpoint_id = None;
    if allow_auto_migrate
        && (opts.migrate_legacy
            || (!had_canonical_memory && ws_dir.join(LEGACY_ROOT_MEMORY_REL).is_file()))
    {
        let mig = migrate_workspace(opts, None)?;
        migrated_legacy_count = mig.migrated_memories.len()
            + mig.migrated_constraints.len()
            + mig.migrated_skills.len()
            + mig.migrated_mcp_servers;
        if mig.checkpoint_id.is_some() {
            checkpoint_id = mig.checkpoint_id;
        }
        changed |= mig.changed;
    }

    // 9. Apply requested harness projections if `--harnesses` was explicitly specified
    let mut final_harnesses = existing_harnesses;
    if let Some(ref spec) = opts.harnesses {
        let sw = switch_harnesses(opts, SwitchMode::To, spec, false, false)?;
        final_harnesses = sw.active_harnesses;
        if sw.checkpoint_id.is_some() && checkpoint_id.is_none() {
            checkpoint_id = sw.checkpoint_id;
        }
        changed |= sw.changed;
    }

    if changed {
        let entry = AuditEntry {
            ts: now,
            action: "workspace-init".to_string(),
            actor: "debopty".to_string(),
            status: "ok".to_string(),
            checkpoint_id: checkpoint_id.clone(),
            summary: format!(
                "initialized/updated agent workspace (created={}, updated={}, preserved={}, migrated={})",
                created_files.len(),
                updated_files.len(),
                preserved_files.len(),
                migrated_legacy_count
            ),
            paths: Vec::new(),
            details: BTreeMap::from([
                (
                    "schema_version".to_string(),
                    WORKSPACE_SCHEMA_VERSION.to_string(),
                ),
                ("debopty_version".to_string(), debopty_version.clone()),
                (
                    "memory_tracking".to_string(),
                    effective_tracking.as_str().to_string(),
                ),
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
        memory_tracking: effective_tracking,
        memory_tracking_notice: memory_tracking_notice(effective_tracking),
        active_harnesses: final_harnesses,
        migrated_legacy_count,
        checkpoint_id,
    })
}

/// Inspect the health, files, memory topics, overrides, blueprint, checkpoints, audit log,
/// and credential hygiene of a workspace.
pub fn workspace_status(
    opts: &AgentWorkspaceOptions,
    runner: Option<&dyn CommandRunner>,
) -> Result<WorkspaceStatusReport> {
    let ws_dir = opts.effective_workspace_dir();
    let debopty_version = current_debopty_version();

    let manifest = load_manifest(&ws_dir);
    let schema_version = manifest.as_ref().map(|m| m.schema_version.clone());
    let wire_epoch = manifest
        .as_ref()
        .map(|m| m.wire_epoch.clone())
        .unwrap_or_else(|| WIRE_EPOCH.to_string());
    let memory_tracking = manifest
        .as_ref()
        .map(|m| m.memory_tracking)
        .unwrap_or_default();
    let active_harnesses = manifest
        .as_ref()
        .map(|m| m.active_harnesses.clone())
        .unwrap_or_default();

    let mut expected: Vec<&str> = MANAGED_DOC_TEMPLATES
        .iter()
        .map(|(_, rel, _)| *rel)
        .collect();
    expected.push(REPO_BLUEPRINT_REL);
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

    let memory_topic_count = memory::discover_topic_files(&ws_dir, false).len();
    let override_count = manage_override(opts, OverrideAction::List)
        .map(|r| r.overrides.len())
        .unwrap_or(0);
    let blueprint_valid =
        load_repo_blueprint(&ws_dir).is_some_and(|bp| matches!(bp.blueprint_schema_version, 1 | 2));
    let legacy_detected = audit_workspace(opts, runner)
        .map(|a| {
            !a.legacy_memory_files.is_empty()
                || !a.legacy_constraint_files.is_empty()
                || !a.legacy_skill_dirs.is_empty()
                || !a.legacy_mcp_files.is_empty()
        })
        .unwrap_or(false);

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
    let initialized = ws_dir.join(WORKSPACE_MANIFEST_REL).is_file() && managed_files_present > 0;
    let strict_perms_ok = check_core_runtime_perms(&ws_dir).is_empty();
    let healthy = initialized
        && missing_files.is_empty()
        && memory_exists
        && blueprint_valid
        && credential_audit.ok
        && strict_perms_ok;

    Ok(WorkspaceStatusReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        initialized,
        schema_version,
        debopty_version,
        wire_epoch,
        memory_tracking,
        active_harnesses,
        managed_files_present,
        managed_files_expected,
        missing_files,
        memory_exists,
        memory_lines,
        memory_topic_count,
        override_count,
        legacy_detected,
        blueprint_valid,
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

/// Roll back all paths captured in `checkpoint_id` (or `"latest"`) onto `opts.root_prefix`
/// and record an audit entry.
pub fn rollback_checkpoint(
    opts: &AgentWorkspaceOptions,
    checkpoint_id: &str,
    actor: &str,
) -> Result<RollbackReport> {
    let resolved_id = if checkpoint_id == "latest" {
        let list = list_checkpoints(opts)?;
        list.into_iter()
            .next()
            .map(|c| c.checkpoint_id)
            .ok_or_else(|| {
                CoreError::Reconcile("no checkpoints available for rollback 'latest'".to_string())
            })?
    } else {
        validate_safe_identifier(checkpoint_id, "checkpoint id")?;
        checkpoint_id.to_string()
    };

    let ws_dir = opts.effective_workspace_dir();
    let run_dir = ws_dir.join(CHECKPOINTS_DIR_REL).join(&resolved_id);
    if !run_dir.is_dir() || !run_dir.join("manifest.json").is_file() {
        return Err(CoreError::Reconcile(format!(
            "checkpoint not found: '{resolved_id}'"
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
        checkpoint_id: Some(resolved_id.clone()),
        summary: format!("rolled back checkpoint '{resolved_id}' ({restored_entries} entries)"),
        paths,
        details: BTreeMap::from([("restored_entries".to_string(), restored_entries.to_string())]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(RollbackReport {
        checkpoint_id: resolved_id,
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

fn check_core_runtime_perms(ws_dir: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    let dir_checks = [
        (MEMORY_DIR_REL, 0o700),
        (MEMORY_ARCHIVE_DIR_REL, 0o700),
        (CREDENTIALS_DIR_REL, CRED_DIR_MODE),
        (CHECKPOINTS_DIR_REL, 0o700),
        (LOGS_DIR_REL, 0o700),
        (CACHE_DIR_REL, 0o700),
        (TMP_DIR_REL, 0o700),
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
        (HOST_PROFILE_REL, CRED_FILE_MODE),
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

    let idx_cache = ws_dir.join(MEMORY_INDEX_CACHE_REL);
    if idx_cache.is_file()
        && let Ok(meta) = fs::metadata(&idx_cache)
    {
        let actual = meta.permissions().mode() & 0o7777;
        if actual != CRED_FILE_MODE {
            errors.push(format!(
                "{MEMORY_INDEX_CACHE_REL}: expected file mode 0600, got {actual:04o}"
            ));
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
    RE.get_or_init(|| {
        Regex::new(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b").expect("valid iso date regex")
    })
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

pub(crate) struct ParsedFrontmatter {
    pub strings: BTreeMap<String, String>,
    pub lists: BTreeMap<String, Vec<String>>,
    pub end_idx: usize,
}

pub(crate) fn parse_frontmatter(lines: &[&str]) -> std::result::Result<ParsedFrontmatter, String> {
    if lines.first().map(|l| l.trim()) != Some("---") {
        return Err("line 1 must be '---'".to_string());
    }
    let Some(end_idx) = lines
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(i, l)| if l.trim() == "---" { Some(i) } else { None })
    else {
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

fn check_agent_only_file(rel_path: &str, ws_dir: &Path) -> (Vec<String>, Option<Vec<String>>) {
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
            errors.push(format!(
                "{rel_path}:{lineno}: <zh_summary> tag forbidden in body"
            ));
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
            if matches!(stripped, MANAGED_START_MARKER | MANAGED_END_MARKER) {
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
        rel_path.starts_with("docs/agent/constraints/") || rel_path.starts_with(".agents/memory/");

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

/// Verify dual-audience format contracts, bidirectional links, date ban, JSON schemas,
/// vendor neutrality, CoALA memory lint, strict `0700`/`0600` runtime perms, and U29 credential audit.
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
    for (rel, _) in memory::discover_topic_files(&ws_dir, false) {
        checks_run += 1;
        errors.extend(check_human_facing_file(&rel, &ws_dir));
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

    // 4. Zero YYYY-MM-DD dates in .agents/CONSTRAINTS.md and canonical .agents/constraints/00..50.
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

    // 5. Valid JSON in `.agents/schemas/*.json` & `.agents/workspace.json`, valid `repo_blueprint.json`,
    //    and zero vendor brand lock-in in generic canonical `.md` files.
    {
        checks_run += 1;
        let schemas_dir = ws_dir.join(SCHEMAS_DIR_REL);
        if schemas_dir.is_dir()
            && let Ok(rd) = fs::read_dir(&schemas_dir)
        {
            for entry in rd.flatten() {
                let p = entry.path();
                if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("json") {
                    match fs::read_to_string(&p)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                    {
                        Some(_) => {}
                        None => errors.push(format!("{}: invalid JSON", p.display())),
                    }
                }
            }
        }
        match load_manifest(&ws_dir) {
            Some(m) if matches!(m.schema_version.as_str(), "1" | "2") => {}
            Some(m) => errors.push(format!(
                "{WORKSPACE_MANIFEST_REL}: unsupported schema_version '{}'",
                m.schema_version
            )),
            None => errors.push(format!("{WORKSPACE_MANIFEST_REL}: missing or invalid JSON")),
        }
        match load_repo_blueprint(&ws_dir) {
            Some(bp) if matches!(bp.blueprint_schema_version, 1 | 2) => {}
            Some(bp) => errors.push(format!(
                "{REPO_BLUEPRINT_REL}: unsupported blueprint_schema_version '{}'",
                bp.blueprint_schema_version
            )),
            None => errors.push(format!("{REPO_BLUEPRINT_REL}: missing or invalid JSON")),
        }
        for &rel in VENDOR_NEUTRAL_FILES {
            let abs = ws_dir.join(rel);
            if let Ok(content) = fs::read_to_string(&abs)
                && let Some(tok) = migrate::find_vendor_lockin_token(&content)
            {
                errors.push(format!(
                    "{rel}: vendor brand token '{tok}' forbidden in generic canonical doc"
                ));
            }
        }
    }

    // 6. CoALA memory lint (TOC cap, topic frontmatter/caps/perms, >15 overflow catalog).
    {
        checks_run += 1;
        let mem_lint = lint_memory(opts)?;
        errors.extend(mem_lint.errors);
        warnings.extend(mem_lint.warnings);
    }

    // 7. Strict 0700/0600 permissions on runtime directories & files.
    checks_run += 1;
    errors.extend(check_core_runtime_perms(&ws_dir));

    // 8. U29 credential audit + workspace-wide secret leak scan outside `.agents/credentials/`.
    checks_run += 1;
    let cred_dir = ws_dir.join(CREDENTIALS_DIR_REL);
    let cred_report =
        credentials::audit_credentials(&cred_dir, std::slice::from_ref(&ws_dir), runner);
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
