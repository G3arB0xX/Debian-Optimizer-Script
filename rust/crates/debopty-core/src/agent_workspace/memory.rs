use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use regex::Regex;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::apps::self_manage::sha256_hex;
use crate::credentials::CRED_FILE_MODE;
use crate::diag;
use crate::error::{
    CoreError, EXIT_MEMORY_BLOAT, EXIT_MEMORY_SECRET_LEAK, EXIT_WRITE_GATE_REJECT, Result,
};
use crate::sys;

use super::{
    AgentWorkspaceOptions, AuditEntry, CACHE_DIR_REL, HOST_PROFILE_REL, MANAGED_END_MARKER,
    MANAGED_START_MARKER, MEMORY_FILE_REL, MEMORY_INDEX_CACHE_REL, append_audit_entry,
    create_checkpoint, ensure_dir_mode, extract_managed_block_any, parse_frontmatter,
    ws_rel_to_checkpoint_path,
};

pub const MEMORY_DIR_REL: &str = ".agents/memory";
pub const MEMORY_ARCHIVE_DIR_REL: &str = ".agents/memory/archive";
pub const MEMORY_INDEX_MD_REL: &str = ".agents/memory/INDEX.md";

pub const TOC_MAX_LINES: usize = 150;
pub const TOC_MAX_BYTES: usize = 12 * 1024;
pub const TOPIC_MAX_LINES: usize = 200;
pub const TOPIC_MAX_BYTES: usize = 16 * 1024;
pub const RAW_FENCE_MAX_LINES: usize = 80;
pub const DEFAULT_SHARD_SUBDIR_THRESHOLD: usize = 15;
pub const MAX_SEARCH_SNIPPET_LINES: usize = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryTracking {
    #[default]
    Local,
    Tracked,
}

impl MemoryTracking {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Tracked => "tracked",
        }
    }
}

impl std::fmt::Display for MemoryTracking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for MemoryTracking {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "tracked" => Ok(Self::Tracked),
            other => Err(CoreError::ValidationFailed(format!(
                "invalid memory-tracking policy '{other}' (expected 'local' or 'tracked')"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TopicType {
    Episodic,
    Semantic,
    Procedural,
    Profile,
}

impl TopicType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Episodic => "episodic",
            Self::Semantic => "semantic",
            Self::Procedural => "procedural",
            Self::Profile => "profile",
        }
    }
}

impl std::fmt::Display for TopicType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TopicType {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "episodic" => Ok(Self::Episodic),
            "semantic" => Ok(Self::Semantic),
            "procedural" => Ok(Self::Procedural),
            "profile" => Ok(Self::Profile),
            other => Err(CoreError::agent_policy(
                EXIT_WRITE_GATE_REJECT,
                "WRITE_GATE_REJECT",
                format!(
                    "invalid CoALA topic type '{other}' (expected episodic|semantic|procedural|profile)"
                ),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TopicStatus {
    #[default]
    Active,
    Superseded,
    Promoted,
    Archived,
}

impl TopicStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Superseded => "superseded",
            Self::Promoted => "promoted",
            Self::Archived => "archived",
        }
    }
}

impl std::fmt::Display for TopicStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for TopicStatus {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "active" => Ok(Self::Active),
            "superseded" => Ok(Self::Superseded),
            "promoted" => Ok(Self::Promoted),
            "archived" => Ok(Self::Archived),
            other => Err(CoreError::agent_policy(
                EXIT_WRITE_GATE_REJECT,
                "WRITE_GATE_REJECT",
                format!(
                    "invalid CoALA topic status '{other}' (expected active|superseded|promoted|archived)"
                ),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTopicInput {
    pub id: String,
    pub title: String,
    pub topic_type: TopicType,
    #[serde(default)]
    pub status: TopicStatus,
    pub scope: String,
    pub tags: Vec<String>,
    pub sources: Vec<String>,
    pub confidence: f64,
    pub body: String,
    #[serde(default)]
    pub ttl_days: Option<u32>,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub supersedes: Option<String>,
    #[serde(default)]
    pub allow_update: bool,
    #[serde(default)]
    pub actor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryTopicMeta {
    pub id: String,
    pub title: String,
    #[serde(rename = "type")]
    pub topic_type: TopicType,
    pub status: TopicStatus,
    pub scope: String,
    pub created_at: String,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub sources: Vec<String>,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_days: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub managed_by: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rel_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum WriteGateVerdict {
    Add,
    Update,
    Supersede,
    Noop,
    DuplicateConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteGateReport {
    pub verdict: WriteGateVerdict,
    pub topic_id: String,
    pub rel_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub changed: bool,
    pub toc_updated: bool,
    pub index_updated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryIndexReport {
    pub index_path: String,
    pub total_topics: usize,
    pub active_topics: usize,
    pub archived_topics: usize,
    pub rebuilt_entries: usize,
    pub reused_entries: usize,
    pub toc_lines: usize,
    pub toc_bytes: usize,
    pub overflow_catalog_active: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchHit {
    pub topic_id: String,
    pub title: String,
    pub rel_path: String,
    pub scope: String,
    pub topic_type: TopicType,
    pub status: TopicStatus,
    pub score: f64,
    pub pointer: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySearchReport {
    pub query: String,
    pub total_hits: usize,
    pub hits: Vec<MemorySearchHit>,
    pub index_rebuilt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidatePlanItem {
    pub action: String,
    pub topic_id: String,
    pub rel_path: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsolidateReport {
    pub applied: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub items: Vec<ConsolidatePlanItem>,
    pub archived_count: usize,
    pub promotion_candidates: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromoteReport {
    pub topic_id: String,
    pub target_doc: String,
    pub archive_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    pub promoted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryLintReport {
    pub passed: bool,
    pub toc_lines: usize,
    pub toc_bytes: usize,
    pub topic_count: usize,
    pub active_topic_count: usize,
    pub index_fresh: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IndexedTopicDoc {
    rel_path: String,
    mtime_secs: u64,
    size_bytes: u64,
    sha256: String,
    meta: MemoryTopicMeta,
    doc_len: usize,
    terms: BTreeMap<String, u32>,
    line_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryIndexCache {
    schema_version: u32,
    generated_at: String,
    doc_count: usize,
    avg_doc_len: f64,
    docs: BTreeMap<String, IndexedTopicDoc>,
}

fn topic_slug_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z0-9][a-z0-9._-]{0,79}$").expect("valid slug regex"))
}

fn secret_patterns() -> &'static [(&'static str, Regex)] {
    static PATTERNS: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            (
                "pem_private_key",
                Regex::new(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("pem regex"),
            ),
            (
                "github_token",
                Regex::new(r"\bgh[pousr]_[A-Za-z0-9]{16,}\b").expect("gh regex"),
            ),
            (
                "aws_access_key",
                Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("aws regex"),
            ),
            (
                "api_sk_token",
                Regex::new(r"\bsk-[A-Za-z0-9_-]{16,}\b").expect("sk regex"),
            ),
            (
                "slack_token",
                Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").expect("slack regex"),
            ),
            (
                "jwt_token",
                Regex::new(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b")
                    .expect("jwt regex"),
            ),
            (
                "url_credentials",
                Regex::new(r"https?://[^\s:/@]+:[^\s/@]+@[^\s/]+").expect("url cred regex"),
            ),
            (
                "inline_secret_assignment",
                Regex::new(
                    r#"(?i)\b(?:password|passwd|api_key|apikey|secret_key|access_token|private_token)\s*[:=]\s*["']?[A-Za-z0-9_./+=-]{8,}["']?"#,
                )
                .expect("assign regex"),
            ),
        ]
    })
}

/// Detect plaintext secrets in arbitrary text without echoing the secret material.
pub(crate) fn detect_secret_leak(text: &str) -> Option<&'static str> {
    for (name, re) in secret_patterns() {
        if let Some(m) = re.find(text) {
            let matched = m.as_str();
            if matched.contains("[REDACTED]")
                || matched.contains("{{")
                || matched.contains("${")
                || matched.contains('<')
            {
                continue;
            }
            return Some(name);
        }
    }
    if diag::redact_sensitive(text) != text {
        return Some("redacted_sensitive_pattern");
    }
    None
}

fn max_fenced_block_lines(body: &str) -> usize {
    let mut in_fence = false;
    let mut fence_marker = "";
    let mut cur_len = 0usize;
    let mut max_len = 0usize;
    for line in body.lines() {
        let trimmed = line.trim_start();
        if !in_fence {
            if trimmed.starts_with("```") {
                in_fence = true;
                fence_marker = "```";
                cur_len = 0;
            } else if trimmed.starts_with("~~~") {
                in_fence = true;
                fence_marker = "~~~";
                cur_len = 0;
            }
        } else if trimmed.starts_with(fence_marker) {
            if cur_len > max_len {
                max_len = cur_len;
            }
            in_fence = false;
            cur_len = 0;
        } else {
            cur_len += 1;
        }
    }
    if in_fence && cur_len > max_len {
        max_len = cur_len;
    }
    max_len
}

fn yaml_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

pub(crate) fn render_topic_markdown(meta: &MemoryTopicMeta, body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("id: {}\n", meta.id));
    out.push_str(&format!("title: {}\n", yaml_quote(&meta.title)));
    out.push_str(&format!("type: {}\n", meta.topic_type.as_str()));
    out.push_str(&format!("status: {}\n", meta.status.as_str()));
    out.push_str(&format!("scope: {}\n", yaml_quote(&meta.scope)));
    out.push_str(&format!("created_at: {}\n", yaml_quote(&meta.created_at)));
    out.push_str(&format!("updated_at: {}\n", yaml_quote(&meta.updated_at)));
    let tags_inline = meta
        .tags
        .iter()
        .map(|t| yaml_quote(t))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("tags: [{tags_inline}]\n"));
    let sources_inline = meta
        .sources
        .iter()
        .map(|s| yaml_quote(s))
        .collect::<Vec<_>>()
        .join(", ");
    out.push_str(&format!("sources: [{sources_inline}]\n"));
    out.push_str(&format!("confidence: {:.2}\n", meta.confidence));
    if let Some(ttl) = meta.ttl_days {
        out.push_str(&format!("ttl_days: {ttl}\n"));
    }
    if let Some(ref exp) = meta.expires_at {
        out.push_str(&format!("expires_at: {}\n", yaml_quote(exp)));
    }
    if let Some(ref sup) = meta.supersedes {
        out.push_str(&format!("supersedes: {}\n", sup));
    }
    if let Some(ref sup_by) = meta.superseded_by {
        out.push_str(&format!("superseded_by: {}\n", sup_by));
    }
    if let Some(ref prom) = meta.promoted_to {
        out.push_str(&format!("promoted_to: {}\n", yaml_quote(prom)));
    }
    if let Some(ref mgr) = meta.managed_by {
        out.push_str(&format!("managed_by: {}\n", mgr));
    }
    out.push_str("---\n\n");
    out.push_str(body.trim_end());
    out.push('\n');
    out
}

pub(crate) fn parse_topic_file(
    rel_path: &str,
    content: &str,
) -> std::result::Result<(MemoryTopicMeta, String), String> {
    let lines: Vec<&str> = content.lines().collect();
    let fm = parse_frontmatter(&lines)?;

    let req_str = |key: &str| -> std::result::Result<String, String> {
        fm.strings
            .get(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| format!("missing required frontmatter field '{key}'"))
    };

    let id = req_str("id")?;
    if !topic_slug_re().is_match(&id) {
        return Err(format!("invalid topic id slug '{id}'"));
    }
    let title = req_str("title")?;
    let topic_type = TopicType::from_str(&req_str("type")?).map_err(|e| e.to_string())?;
    let status = TopicStatus::from_str(&req_str("status")?).map_err(|e| e.to_string())?;
    let scope = req_str("scope")?;
    let created_at = req_str("created_at")?;
    let updated_at = req_str("updated_at")?;
    let confidence_raw = req_str("confidence")?;
    let confidence: f64 = confidence_raw
        .parse()
        .map_err(|_| format!("invalid confidence float '{confidence_raw}'"))?;
    if !confidence.is_finite() || !(0.0..=1.0).contains(&confidence) {
        return Err(format!("confidence {confidence} out of range 0.0..=1.0"));
    }

    let tags = fm
        .lists
        .get("tags")
        .cloned()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "frontmatter 'tags' must be a non-empty list".to_string())?;
    let sources = fm
        .lists
        .get("sources")
        .cloned()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "frontmatter 'sources' must be a non-empty list".to_string())?;

    let ttl_days = match fm.strings.get("ttl_days") {
        Some(raw) => {
            let v: u32 = raw
                .parse()
                .map_err(|_| format!("invalid ttl_days '{raw}'"))?;
            if v < 1 {
                return Err("ttl_days must be >= 1".to_string());
            }
            Some(v)
        }
        None => None,
    };

    let meta = MemoryTopicMeta {
        id,
        title,
        topic_type,
        status,
        scope,
        created_at,
        updated_at,
        tags,
        sources,
        confidence,
        ttl_days,
        expires_at: fm.strings.get("expires_at").cloned(),
        supersedes: fm.strings.get("supersedes").cloned(),
        superseded_by: fm.strings.get("superseded_by").cloned(),
        promoted_to: fm.strings.get("promoted_to").cloned(),
        managed_by: fm.strings.get("managed_by").cloned(),
        rel_path: rel_path.to_string(),
    };

    let body = if fm.end_idx + 1 < lines.len() {
        lines[fm.end_idx + 1..].join("\n").trim_start().to_string()
    } else {
        String::new()
    };
    Ok((meta, body))
}

fn is_reserved_memory_doc(rel_path: &str) -> bool {
    rel_path == MEMORY_INDEX_MD_REL
        || rel_path == ".agents/memory/README.md"
        || rel_path == HOST_PROFILE_REL
}

/// Discover all topic shard paths under `.agents/memory/` (excluding INDEX/README/host-profile).
pub(crate) fn discover_topic_files(ws_dir: &Path, include_archive: bool) -> Vec<(String, PathBuf)> {
    let mem_dir = ws_dir.join(MEMORY_DIR_REL);
    if !mem_dir.is_dir() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for entry in WalkDir::new(&mem_dir).into_iter().flatten() {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(rel_os) = p.strip_prefix(ws_dir) else {
            continue;
        };
        let rel = rel_os.to_string_lossy().replace('\\', "/");
        if is_reserved_memory_doc(&rel) {
            continue;
        }
        if !include_archive && rel.starts_with(".agents/memory/archive/") {
            continue;
        }
        out.push((rel, p.to_path_buf()));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub(crate) fn list_all_topics(
    ws_dir: &Path,
    include_archive: bool,
) -> Vec<(MemoryTopicMeta, String)> {
    let mut topics = Vec::new();
    for (rel, abs) in discover_topic_files(ws_dir, include_archive) {
        if let Ok(raw) = fs::read_to_string(&abs)
            && let Ok(parsed) = parse_topic_file(&rel, &raw)
        {
            topics.push(parsed);
        }
    }
    topics
}

fn sanitize_domain_slug(scope: &str) -> String {
    let base = scope.split(':').next().unwrap_or("general");
    let mut slug = String::new();
    for ch in base.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_' || ch == ' ') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() || trimmed == "archive" {
        "general".to_string()
    } else {
        trimmed
    }
}

/// Fail-closed CoALA Write Gate: validates secrets (rc=23), schema/provenance/dedup (rc=21),
/// anti-bloat caps (rc=22), writes `0600` shard, updates TOC & BM25 index, and logs redacted audit.
pub fn write_memory_topic(
    opts: &AgentWorkspaceOptions,
    input: &MemoryTopicInput,
) -> Result<WriteGateReport> {
    let ws_dir = opts.effective_workspace_dir();

    // 1. Secret scan (rc=23)
    for field in [
        input.title.as_str(),
        input.scope.as_str(),
        input.body.as_str(),
    ]
    .into_iter()
    .chain(input.tags.iter().map(String::as_str))
    .chain(input.sources.iter().map(String::as_str))
    {
        if let Some(kind) = detect_secret_leak(field) {
            return Err(CoreError::agent_policy(
                EXIT_MEMORY_SECRET_LEAK,
                "MEMORY_SECRET_LEAK",
                format!("plaintext secret ({kind}) rejected by Write Gate"),
            ));
        }
    }

    // 2. Schema & provenance validation (rc=21)
    if !topic_slug_re().is_match(input.id.trim()) {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            format!(
                "invalid topic id '{}' (must match ^[a-z0-9][a-z0-9._-]{{0,79}}$)",
                input.id
            ),
        ));
    }
    if input.title.trim().is_empty() {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "topic title must not be empty",
        ));
    }
    if input.scope.trim().is_empty() {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "topic scope must not be empty",
        ));
    }
    if input.tags.is_empty() || input.tags.iter().any(|t| t.trim().is_empty()) {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "topic tags must be a non-empty list of non-empty strings",
        ));
    }
    if input.sources.is_empty() || input.sources.iter().any(|s| s.trim().is_empty()) {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "topic sources must be a non-empty provenance list",
        ));
    }
    if !input.confidence.is_finite() || !(0.0..=1.0).contains(&input.confidence) {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            format!(
                "topic confidence {} must be within 0.0..=1.0",
                input.confidence
            ),
        ));
    }
    if let Some(ttl) = input.ttl_days
        && ttl < 1
    {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "ttl_days must be >= 1",
        ));
    }
    if input.body.trim().is_empty() {
        return Err(CoreError::agent_policy(
            EXIT_WRITE_GATE_REJECT,
            "WRITE_GATE_REJECT",
            "topic body must not be empty",
        ));
    }

    ensure_dir_mode(&ws_dir.join(MEMORY_DIR_REL), 0o700)?;
    ensure_dir_mode(&ws_dir.join(MEMORY_ARCHIVE_DIR_REL), 0o700)?;

    let existing_topics = list_all_topics(&ws_dir, false);
    let now = diag::utc_rfc3339_now();
    let actor = input.actor.as_deref().unwrap_or("debopty");

    let existing_by_id = existing_topics
        .iter()
        .find(|(m, _)| m.id == input.id)
        .cloned();

    let norm_title = input.title.trim().to_ascii_lowercase();
    let existing_by_title = existing_topics.iter().find(|(m, _)| {
        m.id != input.id
            && m.status == TopicStatus::Active
            && m.title.trim().to_ascii_lowercase() == norm_title
    });

    // 3. Novelty / dedup verdict
    let mut superseded_target: Option<(MemoryTopicMeta, String)> = None;
    let verdict = if let Some(ref sup_id) = input.supersedes {
        let Some(found) = existing_topics
            .iter()
            .find(|(m, _)| m.id == *sup_id)
            .cloned()
        else {
            return Err(CoreError::agent_policy(
                EXIT_WRITE_GATE_REJECT,
                "WRITE_GATE_REJECT",
                format!("supersedes target topic '{sup_id}' not found"),
            ));
        };
        superseded_target = Some(found);
        WriteGateVerdict::Supersede
    } else if let Some((ref old_meta, ref old_body)) = existing_by_id {
        if old_meta.title == input.title.trim()
            && old_meta.scope == input.scope.trim()
            && old_meta.tags == input.tags
            && old_body.trim() == input.body.trim()
        {
            return Ok(WriteGateReport {
                verdict: WriteGateVerdict::Noop,
                topic_id: input.id.clone(),
                rel_path: old_meta.rel_path.clone(),
                checkpoint_id: None,
                changed: false,
                toc_updated: false,
                index_updated: false,
            });
        }
        if input.allow_update {
            WriteGateVerdict::Update
        } else {
            return Err(CoreError::agent_policy(
                EXIT_WRITE_GATE_REJECT,
                "WRITE_GATE_REJECT",
                format!(
                    "duplicate conflict: topic '{}' already exists (set allow_update or supersedes)",
                    input.id
                ),
            ));
        }
    } else if let Some((conflict_meta, _)) = existing_by_title {
        if !input.allow_update {
            return Err(CoreError::agent_policy(
                EXIT_WRITE_GATE_REJECT,
                "WRITE_GATE_REJECT",
                format!(
                    "duplicate conflict: title matches existing active topic '{}'",
                    conflict_meta.id
                ),
            ));
        }
        WriteGateVerdict::Add
    } else {
        WriteGateVerdict::Add
    };

    // Determine destination relative path (domain sub-sharding when >15 active topics)
    let active_count = existing_topics
        .iter()
        .filter(|(m, _)| m.status == TopicStatus::Active)
        .count();
    let rel_path = if let Some((ref old_meta, _)) = existing_by_id {
        old_meta.rel_path.clone()
    } else if active_count >= DEFAULT_SHARD_SUBDIR_THRESHOLD {
        let domain = sanitize_domain_slug(&input.scope);
        let sub_dir = ws_dir.join(MEMORY_DIR_REL).join(&domain);
        ensure_dir_mode(&sub_dir, 0o700)?;
        format!("{MEMORY_DIR_REL}/{domain}/{}.md", input.id)
    } else {
        format!("{MEMORY_DIR_REL}/{}.md", input.id)
    };

    let created_at = existing_by_id
        .as_ref()
        .map(|(m, _)| m.created_at.clone())
        .unwrap_or_else(|| now.clone());
    let ttl_days = input.ttl_days.or_else(|| {
        if input.topic_type == TopicType::Episodic {
            Some(90)
        } else {
            None
        }
    });

    let meta = MemoryTopicMeta {
        id: input.id.trim().to_string(),
        title: input.title.trim().to_string(),
        topic_type: input.topic_type,
        status: input.status,
        scope: input.scope.trim().to_string(),
        created_at,
        updated_at: now.clone(),
        tags: input.tags.iter().map(|s| s.trim().to_string()).collect(),
        sources: input.sources.iter().map(|s| s.trim().to_string()).collect(),
        confidence: input.confidence,
        ttl_days,
        expires_at: input.expires_at.clone(),
        supersedes: input.supersedes.clone(),
        superseded_by: None,
        promoted_to: None,
        managed_by: Some("debopty".to_string()),
        rel_path: rel_path.clone(),
    };

    let rendered = render_topic_markdown(&meta, &input.body);

    // 4. Anti-bloat cap (rc=22)
    let line_count = rendered.lines().count();
    let byte_len = rendered.len();
    let fence_max = max_fenced_block_lines(&input.body);
    if line_count > TOPIC_MAX_LINES {
        return Err(CoreError::agent_policy(
            EXIT_MEMORY_BLOAT,
            "MEMORY_BLOAT",
            format!(
                "topic '{}' exceeds {TOPIC_MAX_LINES} lines (got {line_count})",
                meta.id
            ),
        ));
    }
    if byte_len > TOPIC_MAX_BYTES {
        return Err(CoreError::agent_policy(
            EXIT_MEMORY_BLOAT,
            "MEMORY_BLOAT",
            format!(
                "topic '{}' exceeds {TOPIC_MAX_BYTES} bytes (got {byte_len})",
                meta.id
            ),
        ));
    }
    if fence_max > RAW_FENCE_MAX_LINES {
        return Err(CoreError::agent_policy(
            EXIT_MEMORY_BLOAT,
            "MEMORY_BLOAT",
            format!(
                "topic '{}' contains a fenced code block of {fence_max} lines (max {RAW_FENCE_MAX_LINES})",
                meta.id
            ),
        ));
    }

    // 5. Pre-write checkpoint if mutating existing topic(s)
    let mut cp_paths = Vec::new();
    if existing_by_id.is_some() {
        cp_paths.push(ws_rel_to_checkpoint_path(opts, &rel_path));
    }
    if let Some((ref old_sup, _)) = superseded_target {
        cp_paths.push(ws_rel_to_checkpoint_path(opts, &old_sup.rel_path));
    }
    let checkpoint_id = if !cp_paths.is_empty() {
        let cp = create_checkpoint(
            opts,
            &format!("memory-write-{}", meta.id),
            &format!("pre-write checkpoint for topic '{}'", meta.id),
            &cp_paths,
            actor,
        )?;
        Some(cp.checkpoint_id)
    } else {
        None
    };

    // Write superseded target update first if applicable
    if let Some((mut old_meta, old_body)) = superseded_target {
        old_meta.status = TopicStatus::Superseded;
        old_meta.superseded_by = Some(meta.id.clone());
        old_meta.updated_at = now.clone();
        let old_rendered = render_topic_markdown(&old_meta, &old_body);
        sys::write_file_atomic(
            &ws_dir.join(&old_meta.rel_path),
            old_rendered.as_bytes(),
            Some(CRED_FILE_MODE),
        )?;
    }

    // Atomic 0600 write of topic shard
    let dest_abs = ws_dir.join(&rel_path);
    if let Some(parent) = dest_abs.parent() {
        ensure_dir_mode(parent, 0o700)?;
    }
    sys::write_file_atomic(&dest_abs, rendered.as_bytes(), Some(CRED_FILE_MODE))?;

    // Rebuild TOC + incremental BM25 index
    let idx_report = rebuild_memory_index(opts)?;

    // Append redacted audit entry
    let audit = AuditEntry {
        ts: now,
        action: "memory-write".to_string(),
        actor: actor.to_string(),
        status: "ok".to_string(),
        checkpoint_id: checkpoint_id.clone(),
        summary: format!(
            "write_memory_topic '{}' ({:?}) -> {}",
            meta.id, verdict, rel_path
        ),
        paths: vec![rel_path.clone()],
        details: BTreeMap::from([
            ("topic_id".to_string(), meta.id.clone()),
            ("type".to_string(), meta.topic_type.as_str().to_string()),
            ("scope".to_string(), meta.scope.clone()),
            (
                "active_topics".to_string(),
                idx_report.active_topics.to_string(),
            ),
        ]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(WriteGateReport {
        verdict,
        topic_id: meta.id,
        rel_path,
        checkpoint_id,
        changed: true,
        toc_updated: true,
        index_updated: true,
    })
}

/// Append a redacted note under `## <section>` in `<ws_dir>/.agents/MEMORY.md` (`0600`),
/// preserving the managed TOC block and user text outside it.
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
    if let Some(parent) = mem_path.parent() {
        ensure_dir_mode(parent, 0o755)?;
    }
    let existing = if mem_path.is_file() {
        fs::read_to_string(&mem_path)?
    } else {
        "# 宿主机长期维护记忆（CoALA Tier-2 TOC）\n".to_string()
    };

    // Preserve managed block at the tail if present so notes insert above it
    let (prefix, managed_tail) = if let Some((start, end, _)) = extract_managed_block_any(&existing)
    {
        (
            existing[..start].trim_end().to_string(),
            Some(existing[start..end].to_string()),
        )
    } else {
        (existing, None)
    };

    let target_heading = format!("## {eff_section}");
    let mut lines: Vec<String> = prefix.lines().map(str::to_string).collect();
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
    if let Some(tail) = managed_tail {
        rendered.push_str("\n\n");
        rendered.push_str(&tail);
    }
    rendered.push('\n');

    sys::write_file_atomic(&mem_path, rendered.as_bytes(), Some(CRED_FILE_MODE))?;
    Ok(true)
}

fn render_toc_managed_block(
    active_topics: &[MemoryTopicMeta],
    overflow_active: bool,
    max_inline_entries: usize,
) -> String {
    let mut lines = Vec::new();
    lines.push(MANAGED_START_MARKER.to_string());
    lines.push("## 主题记忆分片索引（自动维护）".to_string());
    lines.push(String::new());
    lines.push(format!(
        "- 当前活跃主题分片：`{}`{}",
        active_topics.len(),
        if overflow_active {
            "（已超过 15 个阈值，完整目录见 [memory/INDEX.md](memory/INDEX.md)）"
        } else {
            ""
        }
    ));
    let shown = active_topics.len().min(max_inline_entries);
    for meta in active_topics.iter().take(shown) {
        let link_rel = meta
            .rel_path
            .strip_prefix(".agents/")
            .unwrap_or(&meta.rel_path);
        lines.push(format!(
            "- [{}]({})（`{}` / `{}`）",
            meta.title,
            link_rel,
            meta.topic_type.as_str(),
            meta.scope
        ));
    }
    if active_topics.len() > shown {
        lines.push(format!(
            "- 其余 `{}` 条主题请查阅 [memory/INDEX.md](memory/INDEX.md) 或执行 `debopty agent memory search`。",
            active_topics.len() - shown
        ));
    }
    lines.push(MANAGED_END_MARKER.to_string());
    lines.join("\n")
}

/// Update `.agents/MEMORY.md` managed TOC block (strictly <=150L / <=12KB)
/// and update `.agents/memory/INDEX.md` overflow catalog when >15 active topics exist.
pub(crate) fn refresh_memory_toc_and_overflow(
    ws_dir: &Path,
    active_topics: &[MemoryTopicMeta],
) -> Result<(usize, usize, bool)> {
    let overflow_active = active_topics.len() > DEFAULT_SHARD_SUBDIR_THRESHOLD;
    let mem_path = ws_dir.join(MEMORY_FILE_REL);
    let existing = if mem_path.is_file() {
        fs::read_to_string(&mem_path)?
    } else {
        "# 宿主机长期维护记忆（CoALA Tier-2 TOC）\n".to_string()
    };

    // Determine how many inline entries fit within TOC_MAX_LINES and TOC_MAX_BYTES
    let mut max_entries = if overflow_active {
        DEFAULT_SHARD_SUBDIR_THRESHOLD
    } else {
        active_topics.len()
    };
    let mut final_doc;
    loop {
        let block = render_toc_managed_block(active_topics, overflow_active, max_entries);
        final_doc = if let Some((start, end, _)) = extract_managed_block_any(&existing) {
            let mut s = String::with_capacity(existing.len() + block.len());
            s.push_str(&existing[..start]);
            s.push_str(&block);
            s.push_str(&existing[end..]);
            s
        } else {
            let mut s = existing.trim_end().to_string();
            s.push_str("\n\n");
            s.push_str(&block);
            s.push('\n');
            s
        };
        if !final_doc.ends_with('\n') {
            final_doc.push('\n');
        }
        if (final_doc.lines().count() <= TOC_MAX_LINES && final_doc.len() <= TOC_MAX_BYTES)
            || max_entries == 0
        {
            break;
        }
        max_entries = max_entries.saturating_sub(3);
    }

    if !mem_path.is_file() || final_doc != existing {
        sys::write_file_atomic(&mem_path, final_doc.as_bytes(), Some(CRED_FILE_MODE))?;
    }

    // Update `.agents/memory/INDEX.md` overflow section (strict Agent-only XML KV format!)
    let index_md_path = ws_dir.join(MEMORY_INDEX_MD_REL);
    if index_md_path.is_file() {
        let orig = fs::read_to_string(&index_md_path)?;
        let mut base_lines: Vec<String> = Vec::new();
        let mut skipping_overflow = false;
        for line in orig.lines() {
            if line.trim() == "OVERFLOW_TOPICS:" {
                skipping_overflow = true;
                // Remove preceding blank line if present
                if base_lines.last().is_some_and(|l| l.trim().is_empty()) {
                    base_lines.pop();
                }
                continue;
            }
            if skipping_overflow {
                if line.trim() == "</index>" {
                    skipping_overflow = false;
                    base_lines.push(line.to_string());
                } else if !line.trim().is_empty() && !line.starts_with("  ") {
                    skipping_overflow = false;
                    base_lines.push(String::new());
                    base_lines.push(line.to_string());
                }
                continue;
            }
            base_lines.push(line.to_string());
        }

        if overflow_active
            && let Some(close_pos) = base_lines.iter().rposition(|l| l.trim() == "</index>")
        {
            let mut insert_block = Vec::new();
            insert_block.push(String::new());
            insert_block.push("OVERFLOW_TOPICS:".to_string());
            for meta in active_topics {
                insert_block.push(format!(
                    "  {}: {} ({} | {})",
                    meta.id,
                    meta.rel_path,
                    meta.topic_type.as_str(),
                    meta.scope
                ));
            }
            base_lines.splice(close_pos..close_pos, insert_block);
        }

        let mut updated_idx = base_lines.join("\n");
        updated_idx.push('\n');
        if updated_idx != orig {
            sys::write_file_atomic(&index_md_path, updated_idx.as_bytes(), Some(0o644))?;
        }
    }

    Ok((final_doc.lines().count(), final_doc.len(), overflow_active))
}

fn is_cjk(c: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&c)
}

/// Tokenize mixed English & Simplified Chinese text into ASCII lowercase words + CJK unigrams & bigrams.
pub(crate) fn tokenize_en_zh(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut ascii_buf = String::new();
    let mut cjk_run: Vec<char> = Vec::new();

    let flush_ascii = |buf: &mut String, out: &mut Vec<String>| {
        if buf.len() >= 2 {
            out.push(std::mem::take(buf));
        } else {
            buf.clear();
        }
    };
    let flush_cjk = |run: &mut Vec<char>, out: &mut Vec<String>| {
        for &c in run.iter() {
            out.push(c.to_string());
        }
        for w in run.windows(2) {
            let mut bg = String::with_capacity(8);
            bg.push(w[0]);
            bg.push(w[1]);
            out.push(bg);
        }
        run.clear();
    };

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            flush_cjk(&mut cjk_run, &mut tokens);
            ascii_buf.push(ch.to_ascii_lowercase());
        } else if is_cjk(ch) {
            flush_ascii(&mut ascii_buf, &mut tokens);
            cjk_run.push(ch);
        } else {
            flush_ascii(&mut ascii_buf, &mut tokens);
            flush_cjk(&mut cjk_run, &mut tokens);
        }
    }
    flush_ascii(&mut ascii_buf, &mut tokens);
    flush_cjk(&mut cjk_run, &mut tokens);
    tokens
}

fn file_mtime_size(path: &Path) -> (u64, u64) {
    let Ok(meta) = fs::metadata(path) else {
        return (0, 0);
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (mtime, meta.len())
}

fn load_index_cache(ws_dir: &Path) -> Option<MemoryIndexCache> {
    let p = ws_dir.join(MEMORY_INDEX_CACHE_REL);
    fs::read_to_string(&p)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
}

fn is_index_cache_fresh(ws_dir: &Path, cache: &MemoryIndexCache) -> bool {
    let disk_files = discover_topic_files(ws_dir, true);
    if disk_files.len() != cache.docs.len() {
        return false;
    }
    for (rel, abs) in disk_files {
        let Some(entry) = cache.docs.get(&rel) else {
            return false;
        };
        let (mtime, size) = file_mtime_size(&abs);
        if entry.mtime_secs != mtime || entry.size_bytes != size {
            return false;
        }
    }
    true
}

/// Incrementally rebuild `.agents/cache/memory_index.json` (`0600`) using `(mtime, size, sha256)`
/// and refresh the `.agents/MEMORY.md` managed TOC + `.agents/memory/INDEX.md` overflow catalog.
pub fn rebuild_memory_index(opts: &AgentWorkspaceOptions) -> Result<MemoryIndexReport> {
    let ws_dir = opts.effective_workspace_dir();
    ensure_dir_mode(&ws_dir.join(CACHE_DIR_REL), 0o700)?;

    let prev_cache = load_index_cache(&ws_dir);
    let disk_files = discover_topic_files(&ws_dir, true);

    let mut new_docs: BTreeMap<String, IndexedTopicDoc> = BTreeMap::new();
    let mut rebuilt_entries = 0usize;
    let mut reused_entries = 0usize;
    let mut active_metas = Vec::new();
    let mut archived_topics = 0usize;

    for (rel, abs) in disk_files {
        let (mtime_secs, size_bytes) = file_mtime_size(&abs);
        if let Some(ref prev) = prev_cache
            && let Some(cached) = prev.docs.get(&rel)
            && cached.mtime_secs == mtime_secs
            && cached.size_bytes == size_bytes
        {
            if cached.meta.status == TopicStatus::Active
                && !rel.starts_with(".agents/memory/archive/")
            {
                active_metas.push(cached.meta.clone());
            } else {
                archived_topics += 1;
            }
            new_docs.insert(rel, cached.clone());
            reused_entries += 1;
            continue;
        }

        let raw = fs::read_to_string(&abs)?;
        let sha256 = sha256_hex(raw.as_bytes());
        let Ok((meta, body)) = parse_topic_file(&rel, &raw) else {
            continue;
        };
        let full_text = format!(
            "{} {} {} {}",
            meta.title,
            meta.tags.join(" "),
            meta.scope,
            body
        );
        let tokens = tokenize_en_zh(&full_text);
        let mut terms: BTreeMap<String, u32> = BTreeMap::new();
        for tok in &tokens {
            *terms.entry(tok.clone()).or_insert(0) += 1;
        }
        if meta.status == TopicStatus::Active && !rel.starts_with(".agents/memory/archive/") {
            active_metas.push(meta.clone());
        } else {
            archived_topics += 1;
        }
        new_docs.insert(
            rel.clone(),
            IndexedTopicDoc {
                rel_path: rel,
                mtime_secs,
                size_bytes,
                sha256,
                meta,
                doc_len: tokens.len(),
                terms,
                line_count: raw.lines().count(),
            },
        );
        rebuilt_entries += 1;
    }

    active_metas.sort_by(|a, b| a.id.cmp(&b.id));
    let (toc_lines, toc_bytes, overflow_catalog_active) =
        refresh_memory_toc_and_overflow(&ws_dir, &active_metas)?;

    let doc_count = new_docs.len();
    let total_tokens: usize = new_docs.values().map(|d| d.doc_len).sum();
    let avg_doc_len = if doc_count > 0 {
        total_tokens as f64 / doc_count as f64
    } else {
        1.0
    };

    let cache = MemoryIndexCache {
        schema_version: 1,
        generated_at: diag::utc_rfc3339_now(),
        doc_count,
        avg_doc_len,
        docs: new_docs,
    };
    let mut json = serde_json::to_string_pretty(&cache)
        .map_err(|e| CoreError::System(format!("serialize memory index failed: {e}")))?;
    json.push('\n');
    let index_abs = ws_dir.join(MEMORY_INDEX_CACHE_REL);
    sys::write_file_atomic(&index_abs, json.as_bytes(), Some(CRED_FILE_MODE))?;

    Ok(MemoryIndexReport {
        index_path: MEMORY_INDEX_CACHE_REL.to_string(),
        total_topics: doc_count,
        active_topics: active_metas.len(),
        archived_topics,
        rebuilt_entries,
        reused_entries,
        toc_lines,
        toc_bytes,
        overflow_catalog_active,
    })
}

fn extract_best_snippet(abs_path: &Path, query_tokens: &[String]) -> (usize, usize, String) {
    let Ok(raw) = fs::read_to_string(abs_path) else {
        return (1, 1, String::new());
    };
    let lines: Vec<&str> = raw.lines().collect();
    if lines.is_empty() {
        return (1, 1, String::new());
    }
    let mut best_line = 0usize;
    let mut best_hits = 0usize;
    for (idx, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let hits = query_tokens
            .iter()
            .filter(|q| lower.contains(q.as_str()) || line.contains(q.as_str()))
            .count();
        if hits > best_hits {
            best_hits = hits;
            best_line = idx;
        }
    }
    let start_idx = best_line.saturating_sub(2);
    let end_idx = (start_idx + MAX_SEARCH_SNIPPET_LINES).min(lines.len());
    let snippet = lines[start_idx..end_idx].join("\n");
    (start_idx + 1, end_idx, snippet)
}

/// Search CoALA memory topics via EN+ZH BM25 over `.agents/cache/memory_index.json`
/// (auto-rebuilding if missing or stale), returning folded snippets (`<=20L`) + `path:Lstart-Lend`.
pub fn search_memory(
    opts: &AgentWorkspaceOptions,
    query: &str,
    limit: usize,
) -> Result<MemorySearchReport> {
    let ws_dir = opts.effective_workspace_dir();
    let q_trimmed = query.trim();
    if q_trimmed.is_empty() {
        return Err(CoreError::ValidationFailed(
            "search query must not be empty".to_string(),
        ));
    }

    let mut index_rebuilt = false;
    let cache = match load_index_cache(&ws_dir) {
        Some(c) if is_index_cache_fresh(&ws_dir, &c) => c,
        _ => {
            rebuild_memory_index(opts)?;
            index_rebuilt = true;
            load_index_cache(&ws_dir).unwrap_or(MemoryIndexCache {
                schema_version: 1,
                generated_at: diag::utc_rfc3339_now(),
                doc_count: 0,
                avg_doc_len: 1.0,
                docs: BTreeMap::new(),
            })
        }
    };

    let q_tokens: Vec<String> = {
        let mut seen = BTreeSet::new();
        tokenize_en_zh(q_trimmed)
            .into_iter()
            .filter(|t| seen.insert(t.clone()))
            .collect()
    };
    if q_tokens.is_empty() || cache.docs.is_empty() {
        return Ok(MemorySearchReport {
            query: q_trimmed.to_string(),
            total_hits: 0,
            hits: Vec::new(),
            index_rebuilt,
        });
    }

    let n = cache.docs.len() as f64;
    let avgdl = cache.avg_doc_len.max(1.0);
    let k1 = 1.5_f64;
    let b = 0.75_f64;

    let mut df_map: BTreeMap<&str, usize> = BTreeMap::new();
    for tok in &q_tokens {
        let df = cache
            .docs
            .values()
            .filter(|d| d.terms.contains_key(tok))
            .count();
        df_map.insert(tok.as_str(), df);
    }

    let mut scored: Vec<(f64, &IndexedTopicDoc)> = Vec::new();
    for doc in cache.docs.values() {
        let dl = doc.doc_len.max(1) as f64;
        let mut score = 0.0_f64;
        for tok in &q_tokens {
            let tf = *doc.terms.get(tok).unwrap_or(&0) as f64;
            if tf <= 0.0 {
                continue;
            }
            let df = *df_map.get(tok.as_str()).unwrap_or(&0) as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln().max(0.1);
            score += idf * ((tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avgdl)));
        }
        if score > 0.0 {
            // Slight preference for active topics over archived ones
            if doc.meta.status == TopicStatus::Active {
                score *= 1.15;
            }
            scored.push((score, doc));
        }
    }

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.meta.id.cmp(&b.1.meta.id))
    });

    let total_hits = scored.len();
    let eff_limit = if limit == 0 { 10 } else { limit };
    let mut hits = Vec::new();
    for (score, doc) in scored.into_iter().take(eff_limit) {
        let abs = ws_dir.join(&doc.rel_path);
        let (l_start, l_end, snippet) = extract_best_snippet(&abs, &q_tokens);
        hits.push(MemorySearchHit {
            topic_id: doc.meta.id.clone(),
            title: doc.meta.title.clone(),
            rel_path: doc.rel_path.clone(),
            scope: doc.meta.scope.clone(),
            topic_type: doc.meta.topic_type,
            status: doc.meta.status,
            score: (score * 1000.0).round() / 1000.0,
            pointer: format!("{}:L{l_start}-L{l_end}", doc.rel_path),
            snippet,
        });
    }

    Ok(MemorySearchReport {
        query: q_trimmed.to_string(),
        total_hits,
        hits,
        index_rebuilt,
    })
}

/// Parse YYYY-MM-DD prefix of an RFC3339 timestamp into days since civil epoch (Howard Hinnant's algorithm).
fn rfc3339_to_days(ts: &str) -> Option<i64> {
    let date_part = ts.get(..10)?;
    let mut parts = date_part.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    let y_adj = if m <= 2 { y - 1 } else { y };
    let era = if y_adj >= 0 { y_adj } else { y_adj - 399 } / 400;
    let yoe = y_adj - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

fn is_topic_expired(meta: &MemoryTopicMeta, now_rfc3339: &str) -> bool {
    if let Some(ref exp) = meta.expires_at
        && exp.as_str() <= now_rfc3339
    {
        return true;
    }
    if let Some(ttl) = meta.ttl_days
        && let (Some(created_d), Some(now_d)) = (
            rfc3339_to_days(&meta.created_at),
            rfc3339_to_days(now_rfc3339),
        )
        && now_d - created_d >= i64::from(ttl)
    {
        return true;
    }
    false
}

/// Sleep-time memory consolidation: detects expired, superseded, near-duplicate, oversize,
/// and promotion-candidate topics. With `apply=true`, checkpoints and archives expired/superseded shards.
pub fn consolidate_memory(opts: &AgentWorkspaceOptions, apply: bool) -> Result<ConsolidateReport> {
    let ws_dir = opts.effective_workspace_dir();
    let now = diag::utc_rfc3339_now();
    let active_topics = list_all_topics(&ws_dir, false);

    let mut items = Vec::new();
    let mut archive_targets: Vec<(MemoryTopicMeta, String, bool)> = Vec::new(); // (meta, body, is_expired)
    let mut promotion_candidates = 0usize;

    for (meta, body) in &active_topics {
        if meta.status == TopicStatus::Superseded || meta.superseded_by.is_some() {
            items.push(ConsolidatePlanItem {
                action: "archive_superseded".to_string(),
                topic_id: meta.id.clone(),
                rel_path: meta.rel_path.clone(),
                reason: format!(
                    "superseded by '{}'",
                    meta.superseded_by.as_deref().unwrap_or("newer topic")
                ),
                target: Some(format!("{MEMORY_ARCHIVE_DIR_REL}/{}.md", meta.id)),
            });
            archive_targets.push((meta.clone(), body.clone(), false));
            continue;
        }
        if is_topic_expired(meta, &now) {
            items.push(ConsolidatePlanItem {
                action: "archive_expired".to_string(),
                topic_id: meta.id.clone(),
                rel_path: meta.rel_path.clone(),
                reason: "expired via expires_at or ttl_days".to_string(),
                target: Some(format!("{MEMORY_ARCHIVE_DIR_REL}/{}.md", meta.id)),
            });
            archive_targets.push((meta.clone(), body.clone(), true));
            continue;
        }

        let abs = ws_dir.join(&meta.rel_path);
        if let Ok(raw) = fs::read_to_string(&abs)
            && (raw.lines().count() > TOPIC_MAX_LINES || raw.len() > TOPIC_MAX_BYTES)
        {
            items.push(ConsolidatePlanItem {
                action: "split_oversize".to_string(),
                topic_id: meta.id.clone(),
                rel_path: meta.rel_path.clone(),
                reason: format!(
                    "exceeds cap ({} lines, {} bytes)",
                    raw.lines().count(),
                    raw.len()
                ),
                target: None,
            });
        }

        if meta.status == TopicStatus::Active
            && meta.confidence >= 0.85
            && matches!(meta.topic_type, TopicType::Procedural | TopicType::Semantic)
        {
            promotion_candidates += 1;
            items.push(ConsolidatePlanItem {
                action: "promote_candidate".to_string(),
                topic_id: meta.id.clone(),
                rel_path: meta.rel_path.clone(),
                reason: format!(
                    "high-confidence ({:.2}) {} topic ready for Tier-1 promotion",
                    meta.confidence,
                    meta.topic_type.as_str()
                ),
                target: Some(".agents/constraints/10_system_change_and_style.md".to_string()),
            });
        }
    }

    // Near-duplicate detection across active topics
    for i in 0..active_topics.len() {
        for j in (i + 1)..active_topics.len() {
            let a = &active_topics[i].0;
            let b = &active_topics[j].0;
            if a.status != TopicStatus::Active || b.status != TopicStatus::Active {
                continue;
            }
            let same_title = a.title.trim().eq_ignore_ascii_case(b.title.trim());
            let set_a: BTreeSet<&str> = a.tags.iter().map(String::as_str).collect();
            let set_b: BTreeSet<&str> = b.tags.iter().map(String::as_str).collect();
            let inter = set_a.intersection(&set_b).count();
            let union = set_a.union(&set_b).count();
            let jaccard = if union > 0 {
                inter as f64 / union as f64
            } else {
                0.0
            };
            if same_title || (jaccard >= 0.8 && a.scope == b.scope) {
                items.push(ConsolidatePlanItem {
                    action: "merge_duplicate".to_string(),
                    topic_id: b.id.clone(),
                    rel_path: b.rel_path.clone(),
                    reason: format!("near-duplicate of '{}' (tag_jaccard={jaccard:.2})", a.id),
                    target: Some(a.id.clone()),
                });
            }
        }
    }

    let mut checkpoint_id = None;
    let mut archived_count = 0usize;

    if apply && !archive_targets.is_empty() {
        ensure_dir_mode(&ws_dir.join(MEMORY_ARCHIVE_DIR_REL), 0o700)?;
        let cp_paths: Vec<String> = archive_targets
            .iter()
            .map(|(m, _, _)| ws_rel_to_checkpoint_path(opts, &m.rel_path))
            .collect();
        let cp = create_checkpoint(
            opts,
            "memory-consolidate",
            "pre-consolidation archive checkpoint",
            &cp_paths,
            "debopty",
        )?;
        checkpoint_id = Some(cp.checkpoint_id);

        for (mut meta, body, is_expired) in archive_targets {
            let old_abs = ws_dir.join(&meta.rel_path);
            if is_expired {
                meta.status = TopicStatus::Archived;
            }
            meta.updated_at = now.clone();
            let archive_rel = format!("{MEMORY_ARCHIVE_DIR_REL}/{}.md", meta.id);
            meta.rel_path = archive_rel.clone();
            let rendered = render_topic_markdown(&meta, &body);
            sys::write_file_atomic(
                &ws_dir.join(&archive_rel),
                rendered.as_bytes(),
                Some(CRED_FILE_MODE),
            )?;
            if old_abs.is_file() {
                fs::remove_file(&old_abs)?;
            }
            archived_count += 1;
        }

        rebuild_memory_index(opts)?;

        let audit = AuditEntry {
            ts: now,
            action: "memory-consolidate".to_string(),
            actor: "debopty".to_string(),
            status: "ok".to_string(),
            checkpoint_id: checkpoint_id.clone(),
            summary: format!(
                "consolidated memory (archived={archived_count}, promotion_candidates={promotion_candidates})"
            ),
            paths: Vec::new(),
            details: BTreeMap::from([
                ("archived_count".to_string(), archived_count.to_string()),
                (
                    "promotion_candidates".to_string(),
                    promotion_candidates.to_string(),
                ),
            ]),
        };
        append_audit_entry(&ws_dir, &audit)?;
    }

    Ok(ConsolidateReport {
        applied: apply,
        checkpoint_id,
        items,
        archived_count,
        promotion_candidates,
    })
}

/// Promote a high-confidence CoALA topic into a Tier-1 constraint/doc and archive a tombstone shard.
pub fn promote_memory(
    opts: &AgentWorkspaceOptions,
    topic_id: &str,
    target_doc: &str,
) -> Result<PromoteReport> {
    let ws_dir = opts.effective_workspace_dir();
    if target_doc.trim().is_empty()
        || target_doc.contains("..")
        || target_doc.starts_with('/')
        || target_doc.contains('\\')
    {
        return Err(CoreError::ValidationFailed(format!(
            "invalid promote target_doc '{target_doc}'"
        )));
    }

    let active_topics = list_all_topics(&ws_dir, false);
    let Some((mut meta, body)) = active_topics.into_iter().find(|(m, _)| m.id == topic_id) else {
        return Err(CoreError::ValidationFailed(format!(
            "active memory topic '{topic_id}' not found"
        )));
    };

    let target_rel = if target_doc.starts_with(".agents/") || target_doc.contains('/') {
        target_doc.to_string()
    } else {
        format!(".agents/constraints/{target_doc}")
    };
    let target_abs = ws_dir.join(&target_rel);
    if let Some(parent) = target_abs.parent() {
        ensure_dir_mode(parent, 0o755)?;
    }

    // Prevent introducing YYYY-MM-DD dates into constraint docs during promote
    let sanitized_body = if target_rel.starts_with(".agents/constraints/") {
        let date_re = Regex::new(r"\b20[0-9]{2}-[0-9]{2}-[0-9]{2}\b").expect("date regex");
        date_re.replace_all(&body, "[date-omitted]").into_owned()
    } else {
        body.clone()
    };

    let cp = create_checkpoint(
        opts,
        &format!("memory-promote-{}", meta.id),
        &format!("promote topic '{}' to '{target_rel}'", meta.id),
        &[
            ws_rel_to_checkpoint_path(opts, &meta.rel_path),
            ws_rel_to_checkpoint_path(opts, &target_rel),
        ],
        "debopty",
    )?;

    let mut target_content = if target_abs.is_file() {
        fs::read_to_string(&target_abs)?
    } else {
        format!("# {}\n", meta.title)
    };
    if !target_content.ends_with('\n') {
        target_content.push('\n');
    }
    target_content.push_str(&format!(
        "\n## 晋升规则：{}\n\n- 来源记忆分片：`{}`（`scope: {}`）\n\n{}\n",
        meta.title,
        meta.id,
        meta.scope,
        sanitized_body.trim()
    ));
    sys::write_file_atomic(&target_abs, target_content.as_bytes(), Some(0o644))?;

    // Archive promoted tombstone
    ensure_dir_mode(&ws_dir.join(MEMORY_ARCHIVE_DIR_REL), 0o700)?;
    let old_abs = ws_dir.join(&meta.rel_path);
    let now = diag::utc_rfc3339_now();
    meta.status = TopicStatus::Promoted;
    meta.promoted_to = Some(target_rel.clone());
    meta.updated_at = now.clone();
    let archive_rel = format!("{MEMORY_ARCHIVE_DIR_REL}/{}.md", meta.id);
    meta.rel_path = archive_rel.clone();
    let rendered = render_topic_markdown(&meta, &body);
    sys::write_file_atomic(
        &ws_dir.join(&archive_rel),
        rendered.as_bytes(),
        Some(CRED_FILE_MODE),
    )?;
    if old_abs.is_file() {
        fs::remove_file(&old_abs)?;
    }

    rebuild_memory_index(opts)?;

    let audit = AuditEntry {
        ts: now,
        action: "memory-promote".to_string(),
        actor: "debopty".to_string(),
        status: "ok".to_string(),
        checkpoint_id: Some(cp.checkpoint_id.clone()),
        summary: format!("promoted topic '{}' to '{}'", meta.id, target_rel),
        paths: vec![target_rel.clone(), archive_rel.clone()],
        details: BTreeMap::from([
            ("topic_id".to_string(), meta.id.clone()),
            ("target_doc".to_string(), target_rel.clone()),
        ]),
    };
    append_audit_entry(&ws_dir, &audit)?;

    Ok(PromoteReport {
        topic_id: meta.id,
        target_doc: target_rel,
        archive_path: archive_rel,
        checkpoint_id: Some(cp.checkpoint_id),
        promoted: true,
    })
}

/// Validate CoALA TOC size, all topic shard schemas/caps/perms, overflow catalog, and BM25 index freshness.
pub fn lint_memory(opts: &AgentWorkspaceOptions) -> Result<MemoryLintReport> {
    let ws_dir = opts.effective_workspace_dir();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    let mem_path = ws_dir.join(MEMORY_FILE_REL);
    let (toc_lines, toc_bytes) = if mem_path.is_file() {
        let raw = fs::read_to_string(&mem_path)?;
        let lines = raw.lines().count();
        let bytes = raw.len();
        if lines > TOC_MAX_LINES {
            errors.push(format!(
                "{MEMORY_FILE_REL}: exceeds TOC cap {TOC_MAX_LINES} lines (got {lines})"
            ));
        }
        if bytes > TOC_MAX_BYTES {
            errors.push(format!(
                "{MEMORY_FILE_REL}: exceeds TOC cap {TOC_MAX_BYTES} bytes (got {bytes})"
            ));
        }
        if let Some(kind) = detect_secret_leak(&raw) {
            errors.push(format!(
                "{MEMORY_FILE_REL}: plaintext secret ({kind}) detected in TOC"
            ));
        }
        let mode = fs::metadata(&mem_path)?.permissions().mode() & 0o7777;
        if mode != CRED_FILE_MODE {
            errors.push(format!(
                "{MEMORY_FILE_REL}: expected mode 0600, got {mode:04o}"
            ));
        }
        (lines, bytes)
    } else {
        errors.push(format!("{MEMORY_FILE_REL}: required CoALA TOC missing"));
        (0, 0)
    };

    let all_files = discover_topic_files(&ws_dir, true);
    let mut active_topic_count = 0usize;
    let mut seen_ids = BTreeSet::new();

    for (rel, abs) in &all_files {
        let mode = fs::metadata(abs)?.permissions().mode() & 0o7777;
        if mode != CRED_FILE_MODE {
            errors.push(format!(
                "{rel}: expected topic shard mode 0600, got {mode:04o}"
            ));
        }
        let raw = fs::read_to_string(abs)?;
        if raw.lines().count() > TOPIC_MAX_LINES {
            errors.push(format!(
                "{rel}: exceeds topic cap {TOPIC_MAX_LINES} lines (got {})",
                raw.lines().count()
            ));
        }
        if raw.len() > TOPIC_MAX_BYTES {
            errors.push(format!(
                "{rel}: exceeds topic cap {TOPIC_MAX_BYTES} bytes (got {})",
                raw.len()
            ));
        }
        if let Some(kind) = detect_secret_leak(&raw) {
            errors.push(format!("{rel}: plaintext secret ({kind}) detected"));
        }
        match parse_topic_file(rel, &raw) {
            Ok((meta, body)) => {
                let fence_max = max_fenced_block_lines(&body);
                if fence_max > RAW_FENCE_MAX_LINES {
                    errors.push(format!(
                        "{rel}: fenced code block {fence_max} lines exceeds {RAW_FENCE_MAX_LINES}"
                    ));
                }
                if !rel.starts_with(".agents/memory/archive/") {
                    if !seen_ids.insert(meta.id.clone()) {
                        errors.push(format!("{rel}: duplicate active topic id '{}'", meta.id));
                    }
                    if meta.status == TopicStatus::Active {
                        active_topic_count += 1;
                    }
                }
            }
            Err(e) => {
                errors.push(format!("{rel}: invalid CoALA frontmatter: {e}"));
            }
        }
    }

    if active_topic_count > DEFAULT_SHARD_SUBDIR_THRESHOLD {
        let idx_path = ws_dir.join(MEMORY_INDEX_MD_REL);
        let has_overflow = idx_path.is_file()
            && fs::read_to_string(&idx_path)
                .map(|s| s.contains("OVERFLOW_TOPICS:"))
                .unwrap_or(false);
        if !has_overflow {
            errors.push(format!(
                "{MEMORY_INDEX_MD_REL}: >{DEFAULT_SHARD_SUBDIR_THRESHOLD} active topics ({active_topic_count}) must be cataloged under OVERFLOW_TOPICS:"
            ));
        }
    }

    let index_fresh = match load_index_cache(&ws_dir) {
        Some(cache) => is_index_cache_fresh(&ws_dir, &cache),
        None => all_files.is_empty(),
    };
    if !index_fresh {
        warnings.push(format!(
            "{MEMORY_INDEX_CACHE_REL} is missing or stale; run `debopty agent memory index`"
        ));
    }

    Ok(MemoryLintReport {
        passed: errors.is_empty(),
        toc_lines,
        toc_bytes,
        topic_count: all_files.len(),
        active_topic_count,
        index_fresh,
        errors,
        warnings,
    })
}
