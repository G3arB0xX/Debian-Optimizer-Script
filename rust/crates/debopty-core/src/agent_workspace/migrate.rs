use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::apps::self_manage::sha256_hex;
use crate::credentials::CRED_FILE_MODE;
use crate::diag;
use crate::error::{CoreError, EXIT_SECRET_DETECTED, Result};
use crate::sys::{self, CommandRunner};
use crate::template::TemplateEngine;

use super::adapters::{builtin_adapter_registry, load_repo_blueprint};
use super::memory::{
    MEMORY_DIR_REL, MemoryTopicMeta, MemoryTracking, TopicStatus, TopicType, detect_secret_leak,
    list_all_topics, rebuild_memory_index, render_topic_markdown,
};
use super::{
    AgentWorkspaceOptions, AuditEntry, LEGACY_ROOT_MEMORY_REL, LEGACY_SSOT_REL, MANAGED_END_MARKER,
    MANAGED_START_MARKER, MCP_SERVERS_REL, MEMORY_FILE_REL, SKILLS_DIR_REL,
    WORKSPACE_SCHEMA_VERSION, append_audit_entry, create_checkpoint, ensure_dir_mode,
    extract_managed_block_any, init_workspace_core, load_manifest, memory_tracking_notice,
    save_manifest, ws_rel_to_checkpoint_path,
};

const LEGACY_SPLIT_MAX_LINES: usize = 180;
const LEGACY_SPLIT_MAX_BYTES: usize = 14 * 1024;
const LEGACY_SSOT_TEMPLATE: &str = "templates/agent/constraints/90_project_legacy_ssot.md.tmpl";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyItemKind {
    Memory,
    Constraint,
    Shim,
    Skill,
    McpConfig,
    Drift,
    VendorLockin,
    SecretLeak,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFinding {
    pub kind: LegacyItemKind,
    pub path: String,
    pub severity: String,
    pub message: String,
    pub remediation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceAuditReport {
    pub workspace_dir: String,
    pub standard: bool,
    pub secret_detected: bool,
    pub findings: Vec<AuditFinding>,
    pub legacy_memory_files: Vec<String>,
    pub legacy_constraint_files: Vec<String>,
    pub legacy_skill_dirs: Vec<String>,
    pub legacy_mcp_files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrateReport {
    pub workspace_dir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub migrated_memories: Vec<String>,
    pub migrated_constraints: Vec<String>,
    pub migrated_skills: Vec<String>,
    pub migrated_mcp_servers: usize,
    pub cleaned_legacy_files: Vec<String>,
    pub already_migrated_skipped: usize,
    pub memory_tracking: MemoryTracking,
    pub memory_tracking_notice: String,
    pub changed: bool,
}

fn is_forbidden_credential_filename(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower == ".env"
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower == "id_rsa"
        || lower == "id_ed25519"
        || lower == "credentials.json"
}

fn is_secret_env_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.contains("SECRET")
        || upper.contains("TOKEN")
        || upper.contains("PASSWORD")
        || upper.contains("PASSWD")
        || upper.contains("API_KEY")
        || upper.contains("APIKEY")
        || upper.contains("PRIVATE_KEY")
}

fn is_placeholder_value(val: &str) -> bool {
    let t = val.trim();
    t.is_empty()
        || t.starts_with("${")
        || t.starts_with("{{")
        || (t.starts_with('<') && t.ends_with('>'))
        || t.contains("[REDACTED]")
}

/// Vendor brand tokens forbidden in generic canonical `.md` docs (excluding `90_project_legacy_ssot.md`,
/// `.agents/overrides/`, and harness shims).
pub(crate) fn find_vendor_lockin_token(text: &str) -> Option<&'static str> {
    const TOKENS: &[&str] = &[
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".roorules",
        ".kilorules",
        "claude-code",
        "gemini-cli",
        "copilot-instructions",
        "windsurf",
        "cursor",
        "antigravity",
    ];
    let lower = text.to_ascii_lowercase();
    TOKENS
        .iter()
        .copied()
        .find(|tok| lower.contains(&tok.to_ascii_lowercase()))
}

fn sanitize_slug(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "note".to_string()
    } else if trimmed.len() > 56 {
        trimmed[..56].trim_end_matches('-').to_string()
    } else {
        trimmed
    }
}

fn split_legacy_content(text: &str) -> Vec<String> {
    if text.lines().count() <= LEGACY_SPLIT_MAX_LINES && text.len() <= LEGACY_SPLIT_MAX_BYTES {
        return vec![text.to_string()];
    }
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut cur_lines = 0usize;
    for line in text.lines() {
        let line_bytes = line.len() + 1;
        if cur_lines > 0
            && (cur_lines + 1 > LEGACY_SPLIT_MAX_LINES
                || cur.len() + line_bytes > LEGACY_SPLIT_MAX_BYTES)
        {
            chunks.push(cur);
            cur = String::new();
            cur_lines = 0;
        }
        cur.push_str(line);
        cur.push('\n');
        cur_lines += 1;
    }
    if !cur.trim().is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Upsert a verbatim, SHA-256 deduplicated legacy rule entry into
/// `.agents/constraints/90_project_legacy_ssot.md` (`0644`).
/// Returns `Ok(true)` if inserted, `Ok(false)` if already present (100% idempotent).
pub(crate) fn upsert_legacy_ssot_entry(
    opts: &AgentWorkspaceOptions,
    source_rel: &str,
    verbatim_content: &str,
) -> Result<bool> {
    let ws_dir = opts.effective_workspace_dir();
    let dest = ws_dir.join(LEGACY_SSOT_REL);
    if let Some(parent) = dest.parent() {
        ensure_dir_mode(parent, 0o755)?;
    }

    let existing = if dest.is_file() {
        fs::read_to_string(&dest)?
    } else {
        let tpl =
            TemplateEngine::new(opts.template_dir.clone()).with_root_prefix(&opts.root_prefix);
        tpl.render_to_string(LEGACY_SSOT_TEMPLATE, &BTreeMap::new())?
    };

    let hash = sha256_hex(verbatim_content.as_bytes());
    let marker = format!("<!-- legacy-source-sha256:{hash} -->");
    if existing.contains(&marker) {
        return Ok(false);
    }

    let entry_block = format!(
        "{marker}\n### 来源：`{source_rel}`（`sha256:{short}`）\n\n~~~~text\n{body}\n~~~~\n",
        short = &hash[..12],
        body = verbatim_content.trim_end()
    );

    let updated = if let Some((start, end, block)) = extract_managed_block_any(&existing) {
        let inner = block
            .strip_prefix(MANAGED_START_MARKER)
            .unwrap_or(block)
            .strip_suffix(MANAGED_END_MARKER)
            .unwrap_or(block)
            .replace("<!-- 暂无待收敛的历史规则条目 -->", "")
            .trim()
            .to_string();
        let new_block = if inner.is_empty() {
            format!("{MANAGED_START_MARKER}\n{entry_block}{MANAGED_END_MARKER}")
        } else {
            format!("{MANAGED_START_MARKER}\n{inner}\n\n{entry_block}{MANAGED_END_MARKER}")
        };
        let mut s = String::with_capacity(existing.len() + new_block.len());
        s.push_str(&existing[..start]);
        s.push_str(&new_block);
        s.push_str(&existing[end..]);
        s
    } else {
        format!(
            "{}\n\n{MANAGED_START_MARKER}\n{entry_block}{MANAGED_END_MARKER}\n",
            existing.trim_end()
        )
    };

    let final_text = if updated.ends_with('\n') {
        updated
    } else {
        format!("{updated}\n")
    };
    sys::write_file_atomic(&dest, final_text.as_bytes(), Some(0o644))?;
    Ok(true)
}

/// Read-only workspace audit: scans for secrets (`secret_detected` -> CLI `rc=26`),
/// legacy memories/constraints/shims/skills/MCP configs, projection drift, and vendor lock-in (`rc=24`).
pub fn audit_workspace(
    opts: &AgentWorkspaceOptions,
    _runner: Option<&dyn CommandRunner>,
) -> Result<WorkspaceAuditReport> {
    let ws_dir = opts.effective_workspace_dir();
    let mut findings = Vec::new();
    let mut secret_detected = false;
    let mut legacy_memory_files = Vec::new();
    let mut legacy_constraint_files = Vec::new();
    let mut legacy_skill_dirs = Vec::new();
    let mut legacy_mcp_files = Vec::new();

    if !ws_dir.is_dir() {
        return Ok(WorkspaceAuditReport {
            workspace_dir: ws_dir.to_string_lossy().into_owned(),
            standard: false,
            secret_detected: false,
            findings: vec![AuditFinding {
                kind: LegacyItemKind::Drift,
                path: ws_dir.to_string_lossy().into_owned(),
                severity: "error".to_string(),
                message: "workspace directory does not exist".to_string(),
                remediation: "run `debopty agent init`".to_string(),
            }],
            legacy_memory_files,
            legacy_constraint_files,
            legacy_skill_dirs,
            legacy_mcp_files,
        });
    }

    let bp = load_repo_blueprint(&ws_dir);
    let managed_projs = bp
        .as_ref()
        .map(|b| b.managed_projections.clone())
        .unwrap_or_default();

    // 1. Workspace-wide secret & forbidden credential file scan (excluding .agents/credentials & checkpoints)
    for entry in WalkDir::new(&ws_dir).into_iter().flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Ok(rel_os) = p.strip_prefix(&ws_dir) else {
            continue;
        };
        let rel = rel_os.to_string_lossy().replace('\\', "/");
        if rel.starts_with(".agents/credentials/")
            || rel.starts_with(".agents/checkpoints/")
            || rel.starts_with(".git/")
        {
            continue;
        }
        if let Some(fname) = p.file_name().and_then(|n| n.to_str())
            && is_forbidden_credential_filename(fname)
        {
            secret_detected = true;
            findings.push(AuditFinding {
                kind: LegacyItemKind::SecretLeak,
                path: rel.clone(),
                severity: "error".to_string(),
                message: format!(
                    "forbidden credential file '{fname}' outside .agents/credentials/"
                ),
                remediation:
                    "move secret into `.agents/credentials/` (`0600`) and delete plaintext copy"
                        .to_string(),
            });
            continue;
        }
        if let Ok(meta) = fs::metadata(p)
            && meta.len() <= 2 * 1024 * 1024
            && let Ok(text) = fs::read_to_string(p)
            && let Some(kind) = detect_secret_leak(&text)
        {
            secret_detected = true;
            findings.push(AuditFinding {
                kind: LegacyItemKind::SecretLeak,
                path: rel,
                severity: "error".to_string(),
                message: format!("plaintext secret ({kind}) detected outside .agents/credentials/"),
                remediation: "redact or move secret into `.agents/credentials/` before migrating"
                    .to_string(),
            });
        }
    }

    // 2. Discover legacy memories
    let mut mem_candidates: BTreeSet<String> = BTreeSet::new();
    for single in [LEGACY_ROOT_MEMORY_REL, ".agent/MEMORY.md"] {
        if ws_dir.join(single).is_file() {
            mem_candidates.insert(single.to_string());
        }
    }
    for dir_rel in [".agent/memory", "memory-bank"] {
        let abs = ws_dir.join(dir_rel);
        if abs.is_dir() {
            for entry in WalkDir::new(&abs).into_iter().flatten() {
                let p = entry.path();
                if p.is_file()
                    && p.extension().and_then(|e| e.to_str()) == Some("md")
                    && let Ok(rel_os) = p.strip_prefix(&ws_dir)
                {
                    mem_candidates.insert(rel_os.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }
    for rel in mem_candidates {
        legacy_memory_files.push(rel.clone());
        findings.push(AuditFinding {
            kind: LegacyItemKind::Memory,
            path: rel,
            severity: "warn".to_string(),
            message: "legacy memory file detected outside CoALA `.agents/memory/`".to_string(),
            remediation: "run `debopty agent migrate` for lossless migration".to_string(),
        });
    }

    // 3. Discover legacy constraints & non-canonical shims
    let mut constraint_candidates: BTreeSet<String> = BTreeSet::new();
    let single_shims = [
        "CLAUDE.md",
        "GEMINI.md",
        ".cursorrules",
        ".windsurfrules",
        ".clinerules",
        ".roorules",
        ".kilorules",
        ".github/copilot-instructions.md",
        ".claude/CLAUDE.md",
    ];
    for rel in single_shims {
        let abs = ws_dir.join(rel);
        if !abs.is_file() {
            continue;
        }
        if let Some(proj) = managed_projs.get(rel)
            && let Ok(bytes) = fs::read(&abs)
            && sha256_hex(&bytes) == proj.sha256
        {
            continue; // Clean managed projection
        }
        constraint_candidates.insert(rel.to_string());
    }
    for dir_rel in [
        ".cursor/rules",
        ".windsurf/rules",
        ".clinerules",
        ".roo/rules",
        ".kilocode/rules",
        ".kiro/steering",
        "0_control",
    ] {
        let abs = ws_dir.join(dir_rel);
        if abs.is_dir() {
            for entry in WalkDir::new(&abs).into_iter().flatten() {
                let p = entry.path();
                if !p.is_file() {
                    continue;
                }
                let Ok(rel_os) = p.strip_prefix(&ws_dir) else {
                    continue;
                };
                let rel = rel_os.to_string_lossy().replace('\\', "/");
                if let Some(proj) = managed_projs.get(&rel)
                    && let Ok(bytes) = fs::read(p)
                    && sha256_hex(&bytes) == proj.sha256
                {
                    continue;
                }
                constraint_candidates.insert(rel);
            }
        }
    }
    for rel in constraint_candidates {
        let kind = if matches!(rel.as_str(), "CLAUDE.md" | "GEMINI.md") {
            LegacyItemKind::Shim
        } else {
            LegacyItemKind::Constraint
        };
        legacy_constraint_files.push(rel.clone());
        findings.push(AuditFinding {
            kind,
            path: rel,
            severity: "warn".to_string(),
            message: "unmanaged legacy constraint or harness shim detected".to_string(),
            remediation:
                "run `debopty agent migrate` to preserve verbatim in `90_project_legacy_ssot.md`"
                    .to_string(),
        });
    }
    if ws_dir.join("swarm").is_dir() {
        findings.push(AuditFinding {
            kind: LegacyItemKind::Constraint,
            path: "swarm".to_string(),
            severity: "warn".to_string(),
            message: "stray `swarm/` directory detected in workspace".to_string(),
            remediation: "keep transient swarm ledgers in `/tmp` outside workspace".to_string(),
        });
    }

    // 4. Legacy skills under `.agent/skills/`
    let legacy_skills_root = ws_dir.join(".agent/skills");
    if legacy_skills_root.is_dir()
        && let Ok(rd) = fs::read_dir(&legacy_skills_root)
    {
        for entry in rd.flatten() {
            let p = entry.path();
            if let Ok(rel_os) = p.strip_prefix(&ws_dir) {
                let rel = rel_os.to_string_lossy().replace('\\', "/");
                legacy_skill_dirs.push(rel.clone());
                findings.push(AuditFinding {
                    kind: LegacyItemKind::Skill,
                    path: rel,
                    severity: "warn".to_string(),
                    message: "legacy skill found under `.agent/skills/`".to_string(),
                    remediation: "run `debopty agent migrate` to copy into `.agents/skills/`"
                        .to_string(),
                });
            }
        }
        legacy_skill_dirs.sort();
    }

    // 5. Legacy MCP configs
    let mut mcp_paths: BTreeSet<String> = BTreeSet::from([
        ".mcp.json".to_string(),
        ".cursor/mcp.json".to_string(),
        ".vscode/mcp.json".to_string(),
    ]);
    for adapter in builtin_adapter_registry().adapters.values() {
        for mp in &adapter.mcp_paths {
            if mp.ends_with("mcp.json") {
                mcp_paths.insert(mp.clone());
            }
        }
    }
    for rel in mcp_paths {
        if ws_dir.join(&rel).is_file() {
            legacy_mcp_files.push(rel.clone());
            findings.push(AuditFinding {
                kind: LegacyItemKind::McpConfig,
                path: rel,
                severity: "warn".to_string(),
                message: "legacy MCP configuration file detected".to_string(),
                remediation: "run `debopty agent migrate` to consolidate into `.agents/schemas/mcp_servers.json`".to_string(),
            });
        }
    }

    // 6. Projection drift against repo_blueprint.json
    for (rel, proj) in &managed_projs {
        let abs = ws_dir.join(rel);
        if !abs.is_file() {
            findings.push(AuditFinding {
                kind: LegacyItemKind::Drift,
                path: rel.clone(),
                severity: "warn".to_string(),
                message: "managed harness projection missing on disk".to_string(),
                remediation: "run `debopty agent switch --to <harnesses>` to reconcile".to_string(),
            });
        } else if let Ok(bytes) = fs::read(&abs)
            && sha256_hex(&bytes) != proj.sha256
        {
            findings.push(AuditFinding {
                kind: LegacyItemKind::Drift,
                path: rel.clone(),
                severity: "warn".to_string(),
                message: "managed harness projection modified outside debopty".to_string(),
                remediation: "run `debopty agent switch --harvest` to preserve edits into `90_project_legacy_ssot.md`".to_string(),
            });
        }
    }

    // 7. Vendor lock-in scan in generic canonical `.md` docs
    for rel in [
        "AGENTS.md",
        "README.md",
        ".agents/INDEX.md",
        ".agents/README.md",
        "docs/agent/INDEX.md",
        "docs/agent/README.md",
        "docs/agent/CONSTRAINTS.md",
        "docs/agent/constraints/00_vps_general_and_docs.md",
        "docs/agent/constraints/10_system_change_and_style.md",
        "docs/agent/constraints/20_security_and_credentials.md",
        "docs/agent/constraints/30_atomic_ops_and_audit.md",
        "docs/agent/constraints/40_memory_and_retention.md",
        "docs/agent/constraints/50_harness_decoupling_and_migration.md",
    ] {
        let abs = ws_dir.join(rel);
        if let Ok(content) = fs::read_to_string(&abs)
            && let Some(tok) = find_vendor_lockin_token(&content)
        {
            findings.push(AuditFinding {
                kind: LegacyItemKind::VendorLockin,
                path: rel.to_string(),
                severity: "warn".to_string(),
                message: format!("vendor brand token '{tok}' found in generic canonical doc"),
                remediation:
                    "move vendor-specific instructions into `.agents/overrides/<harness>.md`"
                        .to_string(),
            });
        }
    }

    let standard = findings.is_empty();
    Ok(WorkspaceAuditReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        standard,
        secret_detected,
        findings,
        legacy_memory_files,
        legacy_constraint_files,
        legacy_skill_dirs,
        legacy_mcp_files,
    })
}

fn copy_dir_non_destructive(src: &Path, dst: &Path) -> Result<bool> {
    let mut copied_any = false;
    if src.is_file() {
        if !dst.exists() {
            if let Some(parent) = dst.parent() {
                ensure_dir_mode(parent, 0o755)?;
            }
            let bytes = fs::read(src)?;
            sys::write_file_atomic(dst, &bytes, Some(0o644))?;
            copied_any = true;
        }
        return Ok(copied_any);
    }
    ensure_dir_mode(dst, 0o755)?;
    for entry in WalkDir::new(src).into_iter().flatten() {
        let p = entry.path();
        let Ok(rel) = p.strip_prefix(src) else {
            continue;
        };
        let target = dst.join(rel);
        if p.is_dir() {
            ensure_dir_mode(&target, 0o755)?;
        } else if p.is_file() && !target.exists() {
            if let Some(parent) = target.parent() {
                ensure_dir_mode(parent, 0o755)?;
            }
            let bytes = fs::read(p)?;
            sys::write_file_atomic(&target, &bytes, Some(0o644))?;
            copied_any = true;
        }
    }
    Ok(copied_any)
}

fn extract_secret_free_mcp_servers(
    ws_dir: &Path,
    mcp_files: &[String],
) -> Result<BTreeMap<String, serde_json::Value>> {
    let mut merged: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let existing_path = ws_dir.join(MCP_SERVERS_REL);
    if existing_path.is_file()
        && let Ok(raw) = fs::read_to_string(&existing_path)
        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(obj) = val.get("mcpServers").and_then(|v| v.as_object())
    {
        for (k, v) in obj {
            merged.insert(k.clone(), v.clone());
        }
    }

    for rel in mcp_files {
        let abs = ws_dir.join(rel);
        let Ok(raw) = fs::read_to_string(&abs) else {
            continue;
        };
        if let Some(kind) = detect_secret_leak(&raw) {
            return Err(CoreError::agent_policy(
                EXIT_SECRET_DETECTED,
                "SECRET_DETECTED",
                format!("plaintext secret ({kind}) detected in legacy MCP config '{rel}'"),
            ));
        }
        let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        let servers_obj = val
            .get("mcpServers")
            .or_else(|| val.get("servers"))
            .and_then(|v| v.as_object());
        let Some(obj) = servers_obj else {
            continue;
        };
        for (name, srv) in obj {
            if let Some(env_map) = srv.get("env").and_then(|e| e.as_object()) {
                for (ek, ev) in env_map {
                    if let Some(ev_str) = ev.as_str()
                        && is_secret_env_key(ek)
                        && !is_placeholder_value(ev_str)
                    {
                        return Err(CoreError::agent_policy(
                            EXIT_SECRET_DETECTED,
                            "SECRET_DETECTED",
                            format!(
                                "literal secret env '{ek}' in legacy MCP config '{rel}' server '{name}'; replace with placeholder or store in .agents/credentials/"
                            ),
                        ));
                    }
                }
            }
            merged.entry(name.clone()).or_insert_with(|| srv.clone());
        }
    }
    Ok(merged)
}

/// Lossless, idempotent migration from v1/legacy agent workspace layouts into canonical SSOT v2:
/// fail-closed on plaintext secrets (`rc=26`), pre-migration `0700/0600` checkpoint,
/// legacy memory -> `.agents/memory/legacy-*.md`, legacy constraints -> SHA-256 deduplicated
/// `.agents/constraints/90_project_legacy_ssot.md`, skills -> `.agents/skills/`,
/// secret-free MCP -> `.agents/schemas/mcp_servers.json`.
pub fn migrate_workspace(
    opts: &AgentWorkspaceOptions,
    runner: Option<&dyn CommandRunner>,
) -> Result<MigrateReport> {
    let ws_dir = opts.effective_workspace_dir();
    let audit = audit_workspace(opts, runner)?;

    if audit.secret_detected {
        return Err(CoreError::agent_policy(
            EXIT_SECRET_DETECTED,
            "SECRET_DETECTED",
            "plaintext secret or forbidden credential file detected outside .agents/credentials/; migration aborted fail-closed",
        ));
    }

    // Pre-validate MCP files for literal secret env values before mutating anything
    let mcp_servers = if !audit.legacy_mcp_files.is_empty() {
        extract_secret_free_mcp_servers(&ws_dir, &audit.legacy_mcp_files)?
    } else {
        BTreeMap::new()
    };

    // Capture pre-migration checkpoint BEFORE init_workspace_core or any mutation!
    let mut cp_paths: Vec<String> = Vec::new();
    for rel in audit
        .legacy_memory_files
        .iter()
        .chain(audit.legacy_constraint_files.iter())
        .chain(audit.legacy_mcp_files.iter())
    {
        cp_paths.push(ws_rel_to_checkpoint_path(opts, rel));
    }
    for canon in [
        MEMORY_FILE_REL,
        LEGACY_SSOT_REL,
        MCP_SERVERS_REL,
        ".agents/workspace.json",
        ".agents/schemas/repo_blueprint.json",
    ] {
        cp_paths.push(ws_rel_to_checkpoint_path(opts, canon));
    }

    let has_legacy_work = !audit.legacy_memory_files.is_empty()
        || !audit.legacy_constraint_files.is_empty()
        || !audit.legacy_skill_dirs.is_empty()
        || !audit.legacy_mcp_files.is_empty();

    let existing_v1 = load_manifest(&ws_dir).is_some_and(|m| m.schema_version == "1");

    let checkpoint_id = if has_legacy_work || existing_v1 {
        let cp = create_checkpoint(
            opts,
            "workspace_migrate",
            "pre-migration lossless backup checkpoint",
            &cp_paths,
            "debopty",
        )?;
        Some(cp.checkpoint_id)
    } else {
        None
    };

    // Ensure canonical v2 scaffold exists (without recursing into migrate)
    let init_rep = init_workspace_core(opts, false)?;
    let mut changed = init_rep.changed;

    // Collect already-migrated source provenance tags across all active & archived topics
    let existing_topics = list_all_topics(&ws_dir, true);
    let mut existing_sources: BTreeSet<String> = BTreeSet::new();
    for (meta, _) in &existing_topics {
        for src in &meta.sources {
            existing_sources.insert(src.clone());
        }
    }

    let now = diag::utc_rfc3339_now();
    let mut migrated_memories = Vec::new();
    let mut migrated_constraints = Vec::new();
    let mut migrated_skills = Vec::new();
    let mut migrated_mcp_servers = 0usize;
    let mut cleaned_legacy_files = Vec::new();
    let mut already_migrated_skipped = 0usize;
    let mut migrated_hashes: BTreeSet<String> = BTreeSet::new();

    // 1. Migrate legacy memories -> `.agents/memory/legacy-<slug>[-partN].md` (0600)
    ensure_dir_mode(&ws_dir.join(MEMORY_DIR_REL), 0o700)?;
    for rel in &audit.legacy_memory_files {
        let abs = ws_dir.join(rel);
        let Ok(raw) = fs::read_to_string(&abs) else {
            continue;
        };
        let hash = sha256_hex(raw.as_bytes());
        let prov = format!("migrate:{rel}#sha256:{}", &hash[..12]);
        if existing_sources.contains(&prov) {
            already_migrated_skipped += 1;
            continue;
        }
        migrated_hashes.insert(hash.clone());

        let base_slug = if rel == LEGACY_ROOT_MEMORY_REL {
            "root-memory".to_string()
        } else {
            sanitize_slug(rel.trim_end_matches(".md"))
        };
        let chunks = split_legacy_content(&raw);
        let total_parts = chunks.len();
        for (idx, chunk) in chunks.into_iter().enumerate() {
            let topic_id = if total_parts == 1 {
                format!("legacy-{base_slug}")
            } else {
                format!("legacy-{base_slug}-part{}", idx + 1)
            };
            let title = if total_parts == 1 {
                format!("历史记忆迁移：{rel}")
            } else {
                format!("历史记忆迁移：{rel}（第 {}/{total_parts} 部分）", idx + 1)
            };
            let rel_dest = format!("{MEMORY_DIR_REL}/{topic_id}.md");
            let meta = MemoryTopicMeta {
                id: topic_id,
                title,
                topic_type: TopicType::Episodic,
                status: TopicStatus::Active,
                scope: "legacy".to_string(),
                created_at: now.clone(),
                updated_at: now.clone(),
                tags: vec!["legacy".to_string(), "migrated".to_string()],
                sources: vec![prov.clone()],
                confidence: 0.7,
                ttl_days: Some(90),
                expires_at: None,
                supersedes: None,
                superseded_by: None,
                promoted_to: None,
                managed_by: Some("debopty".to_string()),
                rel_path: rel_dest.clone(),
            };
            let rendered = render_topic_markdown(&meta, &chunk);
            sys::write_file_atomic(
                &ws_dir.join(&rel_dest),
                rendered.as_bytes(),
                Some(CRED_FILE_MODE),
            )?;
            migrated_memories.push(rel_dest);
            changed = true;
        }
        existing_sources.insert(prov);
    }

    // 2. Migrate legacy constraints & non-canonical shims -> 90_project_legacy_ssot.md
    for rel in &audit.legacy_constraint_files {
        let abs = ws_dir.join(rel);
        let Ok(raw) = fs::read_to_string(&abs) else {
            continue;
        };
        let hash = sha256_hex(raw.as_bytes());
        if upsert_legacy_ssot_entry(opts, rel, &raw)? {
            migrated_constraints.push(rel.clone());
            migrated_hashes.insert(hash);
            changed = true;
        } else {
            already_migrated_skipped += 1;
        }
    }

    // 3. Migrate legacy skills `.agent/skills/*` -> `.agents/skills/*`
    if !audit.legacy_skill_dirs.is_empty() {
        ensure_dir_mode(&ws_dir.join(SKILLS_DIR_REL), 0o755)?;
        for rel in &audit.legacy_skill_dirs {
            let src = ws_dir.join(rel);
            let Some(name) = src.file_name() else {
                continue;
            };
            let dst = ws_dir.join(SKILLS_DIR_REL).join(name);
            if copy_dir_non_destructive(&src, &dst)? {
                migrated_skills.push(format!("{SKILLS_DIR_REL}/{}", name.to_string_lossy()));
                changed = true;
            } else {
                already_migrated_skipped += 1;
            }
        }
    }

    // 4. Write secret-free `.agents/schemas/mcp_servers.json`
    if !mcp_servers.is_empty() {
        let doc = serde_json::json!({
            "schema_version": 1,
            "updated_at": now,
            "mcpServers": mcp_servers,
        });
        let mut json = serde_json::to_string_pretty(&doc)
            .map_err(|e| CoreError::System(format!("serialize mcp_servers.json failed: {e}")))?;
        json.push('\n');
        let mcp_dest = ws_dir.join(MCP_SERVERS_REL);
        let needs_write = fs::read_to_string(&mcp_dest)
            .map(|old| old != json)
            .unwrap_or(true);
        if needs_write {
            sys::write_file_atomic(&mcp_dest, json.as_bytes(), Some(0o644))?;
            migrated_mcp_servers = mcp_servers.len();
            changed = true;
        }
    }

    // 5. Rebuild CoALA TOC + BM25 index
    if !migrated_memories.is_empty() || !ws_dir.join(".agents/cache/memory_index.json").is_file() {
        rebuild_memory_index(opts)?;
    }

    // 6. Optional cleanup of migrated legacy originals (ONLY after checkpoint & successful writes;
    // NEVER touches `.agents/**` or `AGENTS.md`).
    if opts.cleanup_legacy {
        for rel in audit
            .legacy_memory_files
            .iter()
            .chain(audit.legacy_constraint_files.iter())
            .chain(audit.legacy_mcp_files.iter())
        {
            if rel == "AGENTS.md" || rel.starts_with(".agents/") {
                continue;
            }
            let abs = ws_dir.join(rel);
            if abs.is_file() {
                fs::remove_file(&abs)?;
                cleaned_legacy_files.push(rel.clone());
                changed = true;
            }
        }
    }

    // 7. Persist v2 manifest migration_state
    let effective_tracking = opts
        .memory_tracking
        .or_else(|| load_manifest(&ws_dir).map(|m| m.memory_tracking))
        .unwrap_or_default();

    if let Some(mut manifest) = load_manifest(&ws_dir) {
        let mut all_hashes: BTreeSet<String> = manifest
            .migration_state
            .migrated_source_hashes
            .iter()
            .cloned()
            .collect();
        for h in migrated_hashes {
            all_hashes.insert(h);
        }
        manifest.schema_version = WORKSPACE_SCHEMA_VERSION.to_string();
        manifest.memory_tracking = effective_tracking;
        if existing_v1 {
            manifest.migration_state.migrated_from_v1 = true;
        }
        if changed {
            manifest.updated_at = now.clone();
            manifest.migration_state.last_migrated_at = Some(now.clone());
            if checkpoint_id.is_some() {
                manifest.migration_state.last_checkpoint_id = checkpoint_id.clone();
            }
            manifest.migration_state.migrated_source_hashes = all_hashes.into_iter().collect();
            save_manifest(&ws_dir, &manifest)?;
        }
    }

    // 7b. Record migration ADR entry in `docs/agent/decisions.md`
    if changed {
        let decisions_abs = ws_dir.join(super::DECISIONS_FILE_REL);
        if decisions_abs.is_file() {
            let mut dec_text = fs::read_to_string(&decisions_abs)?;
            if !dec_text.ends_with('\n') {
                dec_text.push('\n');
            }
            dec_text.push_str(&format!(
                "- `{now}`：执行无损迁移（memories={}、constraints={}、skills={}、mcp_servers={}、cleaned={}）。\n",
                migrated_memories.len(),
                migrated_constraints.len(),
                migrated_skills.len(),
                migrated_mcp_servers,
                cleaned_legacy_files.len(),
            ));
            sys::write_file_atomic(&decisions_abs, dec_text.as_bytes(), Some(0o644))?;
        }
    }

    if changed {
        append_audit_entry(
            &ws_dir,
            &AuditEntry {
                ts: now,
                action: "workspace-migrate".to_string(),
                actor: "debopty".to_string(),
                status: "ok".to_string(),
                checkpoint_id: checkpoint_id.clone(),
                summary: format!(
                    "migrated workspace (memories={}, constraints={}, skills={}, mcp_servers={}, cleaned={})",
                    migrated_memories.len(),
                    migrated_constraints.len(),
                    migrated_skills.len(),
                    migrated_mcp_servers,
                    cleaned_legacy_files.len()
                ),
                paths: migrated_memories
                    .iter()
                    .chain(migrated_constraints.iter())
                    .cloned()
                    .collect(),
                details: BTreeMap::from([
                    (
                        "already_migrated_skipped".to_string(),
                        already_migrated_skipped.to_string(),
                    ),
                    (
                        "memory_tracking".to_string(),
                        effective_tracking.as_str().to_string(),
                    ),
                ]),
            },
        )?;
    }

    Ok(MigrateReport {
        workspace_dir: ws_dir.to_string_lossy().into_owned(),
        checkpoint_id,
        migrated_memories,
        migrated_constraints,
        migrated_skills,
        migrated_mcp_servers,
        cleaned_legacy_files,
        already_migrated_skipped,
        memory_tracking: effective_tracking,
        memory_tracking_notice: memory_tracking_notice(effective_tracking),
        changed,
    })
}
