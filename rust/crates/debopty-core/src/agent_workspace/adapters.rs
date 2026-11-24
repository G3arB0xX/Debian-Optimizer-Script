use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::apps::self_manage::sha256_hex;
use crate::diag;
use crate::error::{CoreError, EXIT_MIGRATION_CONFLICT, Result};
use crate::sys;
use crate::template::TemplateEngine;

use super::memory::MemoryTracking;
use super::migrate::upsert_legacy_ssot_entry;
use super::{
    AgentWorkspaceOptions, AuditEntry, HARNESS_ADAPTERS_REL, OVERRIDES_DIR_REL,
    REPO_BLUEPRINT_REL, WIRE_EPOCH, append_audit_entry, create_checkpoint, ensure_dir_mode,
    load_manifest, save_manifest, validate_safe_identifier, ws_rel_to_checkpoint_path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessAdapterSpec {
    pub id: String,
    #[serde(default)]
    pub projection_paths: Vec<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub legacy_sources: Vec<String>,
    #[serde(default)]
    pub mcp_paths: Vec<String>,
}

fn default_format() -> String {
    "markdown-pointer".to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterRegistry {
    #[serde(default = "default_registry_version")]
    pub adapter_registry_schema_version: u32,
    #[serde(default = "default_wire_epoch")]
    pub wire_epoch: String,
    #[serde(default)]
    pub presets: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub adapters: BTreeMap<String, HarnessAdapterSpec>,
    #[serde(default)]
    pub custom_adapters: BTreeMap<String, HarnessAdapterSpec>,
}

fn default_registry_version() -> u32 {
    1
}

fn default_wire_epoch() -> String {
    WIRE_EPOCH.to_string()
}

fn default_override_bay() -> String {
    OVERRIDES_DIR_REL.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedProjection {
    pub sha256: String,
    pub owners: Vec<String>,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoBlueprint {
    pub blueprint_schema_version: u32,
    #[serde(default = "default_wire_epoch")]
    pub wire_epoch: String,
    #[serde(default)]
    pub workspace_dir: String,
    #[serde(default)]
    pub debopty_version: String,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub memory_tracking: MemoryTracking,
    #[serde(default)]
    pub active_harnesses: Vec<String>,
    #[serde(default)]
    pub managed_projections: BTreeMap<String, ManagedProjection>,
    #[serde(default = "default_override_bay")]
    pub override_bay: String,
    #[serde(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SwitchMode {
    To,
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwitchReport {
    pub mode: SwitchMode,
    pub active_harnesses: Vec<String>,
    pub written_projections: Vec<String>,
    pub removed_projections: Vec<String>,
    pub harvested_projections: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverrideAction {
    Init { harness: String, json: bool },
    List,
    Remove { harness: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideReport {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    #[serde(default)]
    pub overrides: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub changed: bool,
}

/// Built-in 14 harness adapter registry (matching `templates/agent/schemas/harness_adapters.json.tmpl`).
pub fn builtin_adapter_registry() -> AdapterRegistry {
    let mut adapters = BTreeMap::new();
    type BuiltinSpec<'a> = (&'a str, &'a [&'a str], &'a str, &'a [&'a str], &'a [&'a str]);
    let specs: &[BuiltinSpec<'_>] = &[
        ("agents", &[], "canonical", &[], &[]),
        (
            "antigravity",
            &[".agent/rules/debopty-agents.md"],
            "markdown-pointer",
            &[".agent/rules", ".agent/MEMORY.md", ".agent/memory"],
            &[],
        ),
        (
            "claude-code",
            &["CLAUDE.md"],
            "agent-entrypoint-shim",
            &["CLAUDE.md", ".claude/CLAUDE.md"],
            &[".mcp.json"],
        ),
        (
            "gemini-cli",
            &["GEMINI.md"],
            "agent-entrypoint-shim",
            &["GEMINI.md"],
            &[".gemini/settings.json"],
        ),
        ("codex", &[], "canonical", &[], &[]),
        ("opencode", &[], "canonical", &[], &[]),
        (
            "cursor",
            &[".cursor/rules/00-debopty-agents.mdc"],
            "mdc-pointer",
            &[".cursorrules", ".cursor/rules"],
            &[".cursor/mcp.json"],
        ),
        (
            "windsurf",
            &[".windsurf/rules/00-debopty-agents.md"],
            "markdown-pointer",
            &[".windsurfrules", ".windsurf/rules"],
            &[],
        ),
        (
            "cline",
            &[".clinerules/00-debopty-agents.md"],
            "markdown-pointer",
            &[".clinerules"],
            &[],
        ),
        (
            "copilot",
            &[".github/copilot-instructions.md"],
            "markdown-pointer",
            &[".github/copilot-instructions.md"],
            &[".vscode/mcp.json"],
        ),
        (
            "roo-code",
            &[".roo/rules/00-debopty-agents.md"],
            "markdown-pointer",
            &[".roorules", ".roo/rules"],
            &[],
        ),
        (
            "kilo-code",
            &[".kilocode/rules/00-debopty-agents.md"],
            "markdown-pointer",
            &[".kilorules", ".kilocode/rules"],
            &[],
        ),
        (
            "kiro",
            &[".kiro/steering/00-debopty-agents.md"],
            "markdown-pointer",
            &[".kiro/steering"],
            &[],
        ),
        (
            "aider",
            &[".aider.conf.yml"],
            "yaml-read-pointer",
            &[".aider.conf.yml"],
            &[],
        ),
    ];

    let mut all_ids = Vec::new();
    for &(id, proj, fmt, legacy, mcp) in specs {
        all_ids.push(id.to_string());
        adapters.insert(
            id.to_string(),
            HarnessAdapterSpec {
                id: id.to_string(),
                projection_paths: proj.iter().map(|s| (*s).to_string()).collect(),
                format: fmt.to_string(),
                legacy_sources: legacy.iter().map(|s| (*s).to_string()).collect(),
                mcp_paths: mcp.iter().map(|s| (*s).to_string()).collect(),
            },
        );
    }

    let mut presets = BTreeMap::new();
    presets.insert("none".to_string(), Vec::new());
    presets.insert(
        "compat".to_string(),
        vec!["claude-code".to_string(), "gemini-cli".to_string()],
    );
    presets.insert(
        "open".to_string(),
        vec![
            "agents".to_string(),
            "codex".to_string(),
            "opencode".to_string(),
        ],
    );
    presets.insert("auto".to_string(), Vec::new());
    presets.insert("all".to_string(), all_ids);

    AdapterRegistry {
        adapter_registry_schema_version: 1,
        wire_epoch: WIRE_EPOCH.to_string(),
        presets,
        adapters,
        custom_adapters: BTreeMap::new(),
    }
}

/// Load effective adapter registry: built-in 14 adapters merged with workspace `harness_adapters.json`
/// `custom_adapters` and optional `--adapter-file <path>`.
pub fn load_adapter_registry(opts: &AgentWorkspaceOptions) -> Result<AdapterRegistry> {
    let mut reg = builtin_adapter_registry();
    let ws_reg_path = opts.effective_workspace_dir().join(HARNESS_ADAPTERS_REL);
    if ws_reg_path.is_file()
        && let Ok(raw) = fs::read_to_string(&ws_reg_path)
        && let Ok(disk_reg) = serde_json::from_str::<AdapterRegistry>(&raw)
    {
        for (k, v) in disk_reg.custom_adapters {
            reg.custom_adapters.insert(k, v);
        }
    }
    if let Some(ref extra_path) = opts.adapter_file {
        let raw = fs::read_to_string(extra_path).map_err(|e| {
            CoreError::InvalidConfig(format!(
                "failed to read adapter file '{}': {e}",
                extra_path.display()
            ))
        })?;
        if let Ok(extra_reg) = serde_json::from_str::<AdapterRegistry>(&raw) {
            for (k, v) in extra_reg.adapters {
                reg.custom_adapters.insert(k, v);
            }
            for (k, v) in extra_reg.custom_adapters {
                reg.custom_adapters.insert(k, v);
            }
        } else if let Ok(single) = serde_json::from_str::<HarnessAdapterSpec>(&raw) {
            reg.custom_adapters.insert(single.id.clone(), single);
        } else if let Ok(map) = serde_json::from_str::<BTreeMap<String, HarnessAdapterSpec>>(&raw) {
            for (k, v) in map {
                reg.custom_adapters.insert(k, v);
            }
        } else {
            return Err(CoreError::InvalidConfig(format!(
                "invalid custom adapter JSON in '{}'",
                extra_path.display()
            )));
        }
    }
    Ok(reg)
}

impl AdapterRegistry {
    pub fn get_adapter(&self, id: &str) -> Option<&HarnessAdapterSpec> {
        self.custom_adapters
            .get(id)
            .or_else(|| self.adapters.get(id))
    }

    pub fn resolve_spec(&self, ws_dir: &Path, spec: &str) -> Result<Vec<String>> {
        let trimmed = spec.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("none") {
            return Ok(Vec::new());
        }
        if trimmed.eq_ignore_ascii_case("auto") {
            let mut detected = Vec::new();
            for (id, adapter) in self.adapters.iter().chain(self.custom_adapters.iter()) {
                let hit = adapter
                    .projection_paths
                    .iter()
                    .chain(adapter.legacy_sources.iter())
                    .any(|rel| ws_dir.join(rel).exists());
                if hit && !detected.contains(id) {
                    detected.push(id.clone());
                }
            }
            detected.sort();
            return Ok(detected);
        }
        if let Some(preset) = self.presets.get(&trimmed.to_ascii_lowercase()) {
            return Ok(preset.clone());
        }

        let mut resolved = Vec::new();
        for token in trimmed.split(',') {
            let id = token.trim();
            if id.is_empty() || id.eq_ignore_ascii_case("none") {
                continue;
            }
            if let Some(preset) = self.presets.get(&id.to_ascii_lowercase()) {
                for p in preset {
                    if !resolved.contains(p) {
                        resolved.push(p.clone());
                    }
                }
                continue;
            }
            if self.get_adapter(id).is_none() {
                return Err(CoreError::ValidationFailed(format!(
                    "unknown harness adapter or preset '{id}'"
                )));
            }
            if !resolved.contains(&id.to_string()) {
                resolved.push(id.to_string());
            }
        }
        Ok(resolved)
    }
}

pub fn load_repo_blueprint(ws_dir: &Path) -> Option<RepoBlueprint> {
    let p = ws_dir.join(REPO_BLUEPRINT_REL);
    fs::read_to_string(&p)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

pub(crate) fn save_repo_blueprint(ws_dir: &Path, bp: &RepoBlueprint) -> Result<()> {
    let mut json = serde_json::to_string_pretty(bp)
        .map_err(|e| CoreError::System(format!("serialize repo_blueprint.json failed: {e}")))?;
    json.push('\n');
    sys::write_file_atomic(&ws_dir.join(REPO_BLUEPRINT_REL), json.as_bytes(), Some(0o644))?;
    Ok(())
}

fn render_projection_content(adapter: &HarnessAdapterSpec, rel_path: &str) -> String {
    match adapter.format.as_str() {
        "agent-entrypoint-shim" => format!(
            "---\nname: {id}-entrypoint\ndescription: Thin harness entrypoint shim forwarding to canonical AGENTS.md.\nzh_summary: 轻量级运行环境入口垫片，统一导入并遵从工作区根目录 `AGENTS.md` 规范，不另立独立规则。\ntags: [entrypoint, shim, {id}]\nindex: [AGENTS.md]\n---\n\n<guide>\nENTRYPOINT:\n  adapter: {id}\n  target: ./AGENTS.md\n  canonical_index: .agents/INDEX.md\n</guide>\n",
            id = adapter.id
        ),
        "mdc-pointer" => "---\ndescription: Thin harness pointer to canonical AGENTS.md\nalwaysApply: true\n---\n\n# Canonical Agent Entrypoint\n\nFollow `./AGENTS.md` and `.agents/INDEX.md` as the single source of truth for this workspace.\n".to_string(),
        "yaml-read-pointer" => "# managed-by-debopty: thin harness pointer\nread:\n  - AGENTS.md\n".to_string(),
        _ => format!(
            "# Canonical Agent Entrypoint ({id})\n\nRead `./AGENTS.md` and `.agents/INDEX.md` as the single source of truth for this workspace.\n\n- Managed projection: `{rel_path}`\n",
            id = adapter.id
        ),
    }
}

fn prune_empty_parent_dirs(ws_dir: &Path, removed_file: &Path) {
    let dot_agents = ws_dir.join(".agents");
    let mut cur = removed_file.parent();
    while let Some(dir) = cur {
        if dir == ws_dir || dir == dot_agents || !dir.starts_with(ws_dir) {
            break;
        }
        let is_empty = fs::read_dir(dir)
            .map(|mut it| it.next().is_none())
            .unwrap_or(false);
        if is_empty {
            let _ = fs::remove_dir(dir);
            cur = dir.parent();
        } else {
            break;
        }
    }
}

/// Switch, add, or remove harness adapter projections with SHA-256 ref-counting,
/// dirty-shim guard (`rc=25`), optional `--harvest` into `90_project_legacy_ssot.md`, and checkpoints.
pub fn switch_harnesses(
    opts: &AgentWorkspaceOptions,
    mode: SwitchMode,
    harnesses_spec: &str,
    harvest: bool,
    force: bool,
) -> Result<SwitchReport> {
    let ws_dir = opts.effective_workspace_dir();
    let registry = load_adapter_registry(opts)?;
    let requested = registry.resolve_spec(&ws_dir, harnesses_spec)?;

    let mut bp = load_repo_blueprint(&ws_dir).unwrap_or_else(|| RepoBlueprint {
        blueprint_schema_version: 2,
        wire_epoch: WIRE_EPOCH.to_string(),
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        debopty_version: super::current_debopty_version(),
        profiles: vec!["vps-ops".to_string(), "memory".to_string()],
        memory_tracking: opts.memory_tracking.unwrap_or_default(),
        active_harnesses: Vec::new(),
        managed_projections: BTreeMap::new(),
        override_bay: OVERRIDES_DIR_REL.to_string(),
        updated_at: diag::utc_rfc3339_now(),
    });

    let mut new_active_set: BTreeSet<String> = match mode {
        SwitchMode::To => requested.into_iter().collect(),
        SwitchMode::Add => {
            let mut s: BTreeSet<String> = bp.active_harnesses.iter().cloned().collect();
            for r in requested {
                s.insert(r);
            }
            s
        }
        SwitchMode::Remove => {
            let rm: BTreeSet<String> = requested.into_iter().collect();
            bp.active_harnesses
                .iter()
                .filter(|h| !rm.contains(*h))
                .cloned()
                .collect()
        }
    };
    new_active_set.remove("none");
    let new_active: Vec<String> = new_active_set.into_iter().collect();

    // Compute desired projection map: rel_path -> (owners, format, content, sha256)
    let mut desired: BTreeMap<String, (Vec<String>, String, String, String)> = BTreeMap::new();
    for id in &new_active {
        if let Some(adapter) = registry.get_adapter(id) {
            for rel in &adapter.projection_paths {
                let entry = desired.entry(rel.clone()).or_insert_with(|| {
                    let content = render_projection_content(adapter, rel);
                    let hash = sha256_hex(content.as_bytes());
                    (Vec::new(), adapter.format.clone(), content, hash)
                });
                if !entry.0.contains(id) {
                    entry.0.push(id.clone());
                }
            }
        }
    }

    // Dirty-shim check on every existing managed projection that will be removed or modified
    let mut dirty_paths: Vec<(String, String)> = Vec::new();
    for (old_rel, old_proj) in &bp.managed_projections {
        let abs = ws_dir.join(old_rel);
        if !abs.is_file() {
            continue;
        }
        let will_remove = !desired.contains_key(old_rel);
        let will_overwrite = desired
            .get(old_rel)
            .is_some_and(|(_, _, _, new_hash)| new_hash != &old_proj.sha256);
        if !will_remove && !will_overwrite {
            continue;
        }
        let disk_bytes = fs::read(&abs)?;
        let disk_hash = sha256_hex(&disk_bytes);
        if disk_hash != old_proj.sha256 {
            if !harvest && !force {
                return Err(CoreError::agent_policy(
                    EXIT_MIGRATION_CONFLICT,
                    "MIGRATION_CONFLICT",
                    format!(
                        "dirty harness shim '{old_rel}' has user edits (pass --harvest to preserve into 90_project_legacy_ssot.md or --force to overwrite)"
                    ),
                ));
            }
            dirty_paths.push((
                old_rel.clone(),
                String::from_utf8_lossy(&disk_bytes).into_owned(),
            ));
        }
    }

    // Determine paths to write/remove
    let mut to_remove: Vec<String> = bp
        .managed_projections
        .keys()
        .filter(|rel| !desired.contains_key(*rel))
        .cloned()
        .collect();
    to_remove.sort();

    let mut to_write: Vec<String> = Vec::new();
    for (rel, (_, _, _, new_hash)) in &desired {
        let abs = ws_dir.join(rel);
        let needs_write = if abs.is_file() {
            let disk_hash = sha256_hex(&fs::read(&abs)?);
            disk_hash != *new_hash
        } else {
            true
        };
        if needs_write {
            to_write.push(rel.clone());
        }
    }

    let changed =
        !to_remove.is_empty() || !to_write.is_empty() || bp.active_harnesses != new_active;

    if !changed {
        return Ok(SwitchReport {
            mode,
            active_harnesses: new_active,
            written_projections: Vec::new(),
            removed_projections: Vec::new(),
            harvested_projections: Vec::new(),
            checkpoint_id: None,
            changed: false,
        });
    }

    // Pre-switch checkpoint
    let mut cp_paths: Vec<String> = to_remove
        .iter()
        .chain(to_write.iter())
        .map(|rel| ws_rel_to_checkpoint_path(opts, rel))
        .collect();
    cp_paths.push(ws_rel_to_checkpoint_path(opts, REPO_BLUEPRINT_REL));
    let cp = create_checkpoint(
        opts,
        "harness-switch",
        &format!("switch harnesses ({mode:?}): {harnesses_spec}"),
        &cp_paths,
        "debopty",
    )?;
    let checkpoint_id = Some(cp.checkpoint_id);

    // Harvest dirty shims verbatim into 90_project_legacy_ssot.md before mutating
    let mut harvested_projections = Vec::new();
    if harvest {
        for (rel, content) in &dirty_paths {
            upsert_legacy_ssot_entry(opts, rel, content)?;
            harvested_projections.push(rel.clone());
        }
    }

    // Remove retired shims & prune empty parent dirs
    for rel in &to_remove {
        let abs = ws_dir.join(rel);
        if abs.is_file() {
            fs::remove_file(&abs)?;
            prune_empty_parent_dirs(&ws_dir, &abs);
        }
    }

    // Write new/updated shims
    for rel in &to_write {
        if let Some((_, _, content, _)) = desired.get(rel) {
            let abs = ws_dir.join(rel);
            if let Some(parent) = abs.parent() {
                ensure_dir_mode(parent, 0o755)?;
            }
            sys::write_file_atomic(&abs, content.as_bytes(), Some(0o644))?;
        }
    }

    // Update repo_blueprint.json & workspace.json
    let now = diag::utc_rfc3339_now();
    bp.active_harnesses = new_active.clone();
    bp.managed_projections = desired
        .into_iter()
        .map(|(rel, (owners, format, _, sha256))| {
            (
                rel,
                ManagedProjection {
                    sha256,
                    owners,
                    format,
                },
            )
        })
        .collect();
    bp.updated_at = now.clone();
    save_repo_blueprint(&ws_dir, &bp)?;

    if let Some(mut manifest) = load_manifest(&ws_dir) {
        manifest.active_harnesses = new_active.clone();
        manifest.updated_at = now.clone();
        save_manifest(&ws_dir, &manifest)?;
    }

    let audit = AuditEntry {
        ts: now,
        action: "harness-switch".to_string(),
        actor: "debopty".to_string(),
        status: "ok".to_string(),
        checkpoint_id: checkpoint_id.clone(),
        summary: format!(
            "switched harnesses ({mode:?}) -> [{}] (written={}, removed={}, harvested={})",
            new_active.join(","),
            to_write.len(),
            to_remove.len(),
            harvested_projections.len()
        ),
        paths: to_write
            .iter()
            .chain(to_remove.iter())
            .cloned()
            .collect(),
        details: BTreeMap::from([
            ("active_harnesses".to_string(), new_active.join(",")),
            ("harvest".to_string(), harvest.to_string()),
        ]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(SwitchReport {
        mode,
        active_harnesses: new_active,
        written_projections: to_write,
        removed_projections: to_remove,
        harvested_projections,
        checkpoint_id,
        changed: true,
    })
}

/// Manage `.agents/overrides/<harness>.{md,json}` plugin override bay without touching canonical docs.
pub fn manage_override(
    opts: &AgentWorkspaceOptions,
    action: OverrideAction,
) -> Result<OverrideReport> {
    let ws_dir = opts.effective_workspace_dir();
    let overrides_dir = ws_dir.join(OVERRIDES_DIR_REL);
    ensure_dir_mode(&overrides_dir, 0o755)?;

    let list_existing = || -> Result<Vec<String>> {
        let mut out = Vec::new();
        if overrides_dir.is_dir() {
            for entry in fs::read_dir(&overrides_dir)? {
                let entry = entry?;
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if matches!(name, "INDEX.md" | "README.md" | "_template.md") {
                    continue;
                }
                if name.ends_with(".md") || name.ends_with(".json") {
                    out.push(format!("{OVERRIDES_DIR_REL}/{name}"));
                }
            }
        }
        out.sort();
        Ok(out)
    };

    match action {
        OverrideAction::List => Ok(OverrideReport {
            action: "list".to_string(),
            target_path: None,
            overrides: list_existing()?,
            checkpoint_id: None,
            changed: false,
        }),
        OverrideAction::Init { harness, json } => {
            validate_safe_identifier(&harness, "override harness id")?;
            let ext = if json { "json" } else { "md" };
            let rel_path = format!("{OVERRIDES_DIR_REL}/{harness}.{ext}");
            let abs_path = ws_dir.join(&rel_path);
            if abs_path.is_file() {
                return Ok(OverrideReport {
                    action: "init".to_string(),
                    target_path: Some(rel_path),
                    overrides: list_existing()?,
                    checkpoint_id: None,
                    changed: false,
                });
            }

            let content = if json {
                format!(
                    "{{\n  \"harness_id\": \"{harness}\",\n  \"workspace_dir\": \"{}\",\n  \"overrides\": {{}}\n}}\n",
                    ws_dir.to_string_lossy()
                )
            } else {
                let tpl = TemplateEngine::new(opts.template_dir.clone())
                    .with_root_prefix(&opts.root_prefix);
                let mut vars = BTreeMap::new();
                vars.insert("HARNESS_ID".to_string(), harness.clone());
                vars.insert(
                    "WORKSPACE_DIR".to_string(),
                    ws_dir.to_string_lossy().into_owned(),
                );
                tpl.render_to_string("templates/agent/overrides/_template.md.tmpl", &vars)?
            };

            sys::write_file_atomic(&abs_path, content.as_bytes(), Some(0o644))?;
            append_audit_entry(
                &ws_dir,
                &AuditEntry {
                    ts: diag::utc_rfc3339_now(),
                    action: "override-init".to_string(),
                    actor: "debopty".to_string(),
                    status: "ok".to_string(),
                    checkpoint_id: None,
                    summary: format!("initialized harness override '{rel_path}'"),
                    paths: vec![rel_path.clone()],
                    details: BTreeMap::from([("harness".to_string(), harness)]),
                },
            )?;

            Ok(OverrideReport {
                action: "init".to_string(),
                target_path: Some(rel_path),
                overrides: list_existing()?,
                checkpoint_id: None,
                changed: true,
            })
        }
        OverrideAction::Remove { harness } => {
            validate_safe_identifier(&harness, "override harness id")?;
            let mut removed_rel = None;
            for ext in ["md", "json"] {
                let rel = format!("{OVERRIDES_DIR_REL}/{harness}.{ext}");
                if ws_dir.join(&rel).is_file() {
                    removed_rel = Some(rel);
                    break;
                }
            }
            let Some(rel_path) = removed_rel else {
                return Ok(OverrideReport {
                    action: "remove".to_string(),
                    target_path: None,
                    overrides: list_existing()?,
                    checkpoint_id: None,
                    changed: false,
                });
            };
            let cp = create_checkpoint(
                opts,
                &format!("override-remove-{harness}"),
                &format!("remove override '{rel_path}'"),
                &[ws_rel_to_checkpoint_path(opts, &rel_path)],
                "debopty",
            )?;
            fs::remove_file(ws_dir.join(&rel_path))?;
            append_audit_entry(
                &ws_dir,
                &AuditEntry {
                    ts: diag::utc_rfc3339_now(),
                    action: "override-remove".to_string(),
                    actor: "debopty".to_string(),
                    status: "ok".to_string(),
                    checkpoint_id: Some(cp.checkpoint_id.clone()),
                    summary: format!("removed harness override '{rel_path}'"),
                    paths: vec![rel_path.clone()],
                    details: BTreeMap::from([("harness".to_string(), harness)]),
                },
            )?;

            Ok(OverrideReport {
                action: "remove".to_string(),
                target_path: Some(rel_path),
                overrides: list_existing()?,
                checkpoint_id: Some(cp.checkpoint_id),
                changed: true,
            })
        }
    }
}
